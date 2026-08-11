//! Subtle keypress rumble via a Linux evdev force-feedback (FF) node.
//!
//! On the phone we target the `spmi_haptics` node so tapping a key produces a
//! short, gentle buzz. But the *same binary* also runs on the x86_64 dev
//! machine and CI, where there is no rumble motor at all. So this module
//! DEGRADES GRACEFULLY: if no suitable FF device is found (the common case off
//! the phone), construction succeeds anyway and every method becomes a silent
//! no-op. WHY this matters: we ship one artifact for both targets — it must
//! never panic, block, or error just because the hardware isn't there.
//!
//! Both construction ([`Haptics::init`]) and every method are infallible: they
//! swallow all device errors and never panic.

use std::cell::RefCell;

/// A live force-feedback handle: the opened evdev device plus the rumble
/// effect uploaded onto it.
///
/// We deliberately keep `device` alive here even though playback goes through
/// `effect`: dropping the `Device` unregisters any effects uploaded to it, so
/// the device MUST outlive the effect. Bundling both in one struct (behind a
/// single `RefCell`) enforces that ownership rule and satisfies the `&mut self`
/// each of them requires at upload/play time.
struct FfState {
    /// The opened FF-capable evdev device. Held only so it outlives `effect`;
    /// dropping it would unregister the uploaded effect below. It is never read
    /// directly — it exists purely as a drop-order guard — hence `allow`.
    #[allow(dead_code)]
    device: evdev::Device,
    /// The pre-uploaded rumble effect we replay on every keypress.
    effect: evdev::FFEffect,
}

/// Keypress haptics with a graceful-degradation contract.
///
/// When an FF device is present (`inner` is `Some`), [`buzz`](Haptics::buzz)
/// replays a short rumble. When absent — the usual case on desktop/CI —
/// `inner` is `None` and every method is a silent no-op. Never panics.
pub struct Haptics {
    /// `Some` once we've opened an FF device and uploaded a rumble effect;
    /// `None` when no device is available. The whole `FfState` sits behind one
    /// `RefCell` because both upload and playback need `&mut` access.
    inner: Option<RefCell<FfState>>,
}

impl Haptics {
    /// Construct the haptics handle, opening an FF device if one exists.
    ///
    /// Never panics, never blocks, never propagates errors: on any failure
    /// (no device, permission denied, upload rejected) it simply yields a
    /// no-op handle whose [`buzz`](Haptics::buzz) does nothing.
    pub fn init() -> Haptics {
        Haptics {
            inner: try_open().map(RefCell::new),
        }
    }

    /// Play a single short rumble, or do nothing when no FF device is present.
    ///
    /// Infallible by contract: a missing device, a device that vanished, or a
    /// re-entrant call all resolve to a silent no-op rather than a panic.
    pub fn buzz(&self) {
        let Some(cell) = self.inner.as_ref() else {
            // No FF device — the common desktop/CI case. Nothing to do.
            return;
        };
        // Use `try_borrow_mut` rather than `borrow_mut`: `buzz` may be invoked
        // from GTK signal handlers, and a nested/re-entrant buzz must not
        // double-borrow-panic. If already borrowed, we simply skip this buzz.
        if let Ok(mut state) = cell.try_borrow_mut() {
            if let Err(e) = state.effect.play(1) {
                // Swallow: the device may have disappeared. A persistent
                // failure just keeps no-opping cheaply on every future call.
                tracing::trace!(error = %e, "haptics: effect play failed");
            }
        }
    }
}

/// Try to open an FF-capable evdev device and upload the rumble effect.
///
/// Returns `None` on any failure so [`Haptics::init`] can degrade to a no-op.
/// Prefers the phone's `spmi_haptics` node by name, otherwise falls back to the
/// first device advertising `FF_RUMBLE`. Never panics: every fallible step uses
/// `?`/`.ok()?`, and logging never influences control flow.
fn try_open() -> Option<FfState> {
    // Single pass over all devices: take the `spmi_haptics` node immediately if
    // we see it, otherwise remember the first FF_RUMBLE-capable device as a
    // fallback. `enumerate()` yields owned Devices, so we can move ours out.
    let mut fallback: Option<evdev::Device> = None;
    let mut named: Option<evdev::Device> = None;

    for (_path, device) in evdev::enumerate() {
        if device.name() == Some("spmi_haptics") {
            named = Some(device);
            break;
        }
        if fallback.is_none() {
            if let Some(set) = device.supported_ff() {
                if set.contains(evdev::FFEffectCode::FF_RUMBLE) {
                    fallback = Some(device);
                }
            }
        }
    }

    let mut device = named.or(fallback)?;
    tracing::debug!(name = ?device.name(), "haptics: using FF device");

    // A short, subtle rumble: ~20 ms, biased toward the strong motor.
    let data = evdev::FFEffectData {
        direction: 0,
        trigger: evdev::FFTrigger::default(),
        replay: evdev::FFReplay { length: 20, delay: 0 }, // ~20 ms, subtle
        kind: evdev::FFEffectKind::Rumble {
            strong_magnitude: 0x8000,
            weak_magnitude: 0x4000,
        },
    };

    let effect = match device.upload_ff_effect(data) {
        Ok(effect) => effect,
        Err(e) => {
            tracing::debug!(error = %e, "haptics: FF effect upload failed");
            return None;
        }
    };

    Some(FfState { device, effect })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs on x86_64 CI/dev with no FF device: `init` + `buzz` must be a
    /// silent no-op and must never panic. (If a real device happens to exist
    /// the buzz is harmless; we only assert the absence of panics.)
    #[test]
    fn init_and_buzz_never_panic() {
        let h = Haptics::init();
        // On the dev machine / CI there is no FF device, so this must be a no-op.
        h.buzz();
        h.buzz(); // idempotent, still no panic
    }
}
