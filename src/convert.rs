//! Pure-Rust unit-conversion core (UI-agnostic).
//!
//! This module knows nothing about GTK. It exposes a small, static,
//! allocation-free surface that a UI layer drives: pick a [`Category`], pick a
//! source and target [`Unit`], call [`convert`]. Currency is deliberately
//! **excluded** (it needs live rates, not fixed constants).
//!
//! # Model
//!
//! Every category except temperature is *linear*: each unit stores a `factor`
//! that is "how many base units one of this unit is worth". Conversion is then
//!
//! ```text
//! value_in_base = value * from.factor
//! result        = value_in_base / to.factor
//! ```
//!
//! The base unit of a category is simply the unit whose `factor == 1.0` (listed
//! first in each table). Temperature is **special**: Celsius, Fahrenheit and
//! Kelvin are related by offsets, not a single multiplicative factor, so
//! [`convert`] matches on [`Category::Temperature`] and routes through Celsius.
//!
//! # Public surface
//!
//! * [`Unit`] — `{ id, name, symbol }`, all `&'static str`.
//! * [`Category`] — the 12 supported categories (unit-like enum, `Copy`).
//!   * [`Category::all`] → `&'static [Category]` (menu order).
//!   * [`Category::name`] → human label, e.g. `"Length"`.
//!   * [`Category::units`] → `&'static [Unit]` for the category.
//!   * [`Category::default_from`] / [`Category::default_to`] → sensible
//!     starting units (`&'static Unit`).
//!   * [`Category::unit_by_id`] → `Option<&'static Unit>` lookup by `id`.
//! * [`convert`]`(cat, from, to, value) -> f64` — the one conversion call.
//! * [`format_conversion`]`(f64) -> String` — display helper; delegates to
//!   [`crate::engine::format_result`] so converted values group / trim / use
//!   E-notation exactly like normal calculator results. The UI may call either
//!   this or `crate::engine::format_result` directly (both are `pub`).
//!
//! `Unit` values handed out by this module are always references into the
//! `static` tables below, so a UI can compare them by `id` (or by pointer) and
//! store them as `&'static Unit` without cloning.

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A single unit within a category.
///
/// `factor` is intentionally **not** public: it is an implementation detail of
/// linear conversion (and meaningless for temperature). The UI only needs the
/// human-facing fields; conversion goes through [`convert`].
pub struct Unit {
    /// Stable, machine-readable identifier (unique within its category), e.g.
    /// `"kilometer"`. Suitable for persistence and equality checks.
    pub id: &'static str,
    /// Human-readable name, e.g. `"Kilometer"`.
    pub name: &'static str,
    /// Short symbol for compact display, e.g. `"km"`.
    pub symbol: &'static str,
    /// How many *base units* one of this unit equals (linear categories only).
    /// Ignored for [`Category::Temperature`].
    factor: f64,
}

/// The supported conversion categories. Currency is deliberately absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Length,
    Area,
    Volume,
    Mass,
    Temperature,
    Speed,
    Time,
    Data,
    Pressure,
    Energy,
    Power,
    Angle,
}

// ---------------------------------------------------------------------------
// Category API
// ---------------------------------------------------------------------------

impl Category {
    /// All categories, in the order a UI should present them.
    pub fn all() -> &'static [Category] {
        &[
            Category::Length,
            Category::Area,
            Category::Volume,
            Category::Mass,
            Category::Temperature,
            Category::Speed,
            Category::Time,
            Category::Data,
            Category::Pressure,
            Category::Energy,
            Category::Power,
            Category::Angle,
        ]
    }

    /// Human-readable category label.
    pub fn name(&self) -> &'static str {
        match self {
            Category::Length => "Length",
            Category::Area => "Area",
            Category::Volume => "Volume",
            Category::Mass => "Mass",
            Category::Temperature => "Temperature",
            Category::Speed => "Speed",
            Category::Time => "Time",
            Category::Data => "Data",
            Category::Pressure => "Pressure",
            Category::Energy => "Energy",
            Category::Power => "Power",
            Category::Angle => "Angle",
        }
    }

    /// The units in this category. The first entry is always the base unit
    /// (`factor == 1.0`) for linear categories.
    pub fn units(&self) -> &'static [Unit] {
        match self {
            Category::Length => LENGTH,
            Category::Area => AREA,
            Category::Volume => VOLUME,
            Category::Mass => MASS,
            Category::Temperature => TEMPERATURE,
            Category::Speed => SPEED,
            Category::Time => TIME,
            Category::Data => DATA,
            Category::Pressure => PRESSURE,
            Category::Energy => ENERGY,
            Category::Power => POWER,
            Category::Angle => ANGLE,
        }
    }

    /// A sensible default source unit (metric/base for most categories).
    pub fn default_from(&self) -> &'static Unit {
        // The base unit — first in every table — is a safe, familiar default.
        &self.units()[0]
    }

    /// A sensible default target unit, chosen to make the default pairing
    /// immediately useful (e.g. meters → feet, °C → °F).
    pub fn default_to(&self) -> &'static Unit {
        let id = match self {
            Category::Length => "foot",
            Category::Area => "square_foot",
            Category::Volume => "milliliter",
            Category::Mass => "pound",
            Category::Temperature => "fahrenheit",
            Category::Speed => "kilometer_per_hour",
            Category::Time => "minute",
            Category::Data => "megabyte",
            Category::Pressure => "bar",
            Category::Energy => "kilocalorie",
            Category::Power => "horsepower",
            Category::Angle => "radian",
        };
        // Fall back to the second unit if an id above is ever mistyped; both are
        // `&'static`, so this stays allocation-free and infallible.
        self.unit_by_id(id).unwrap_or(&self.units()[1])
    }

    /// Look up a unit in this category by its stable `id`.
    pub fn unit_by_id(&self, id: &str) -> Option<&'static Unit> {
        self.units().iter().find(|u| u.id == id)
    }
}

// ---------------------------------------------------------------------------
// Conversion
// ---------------------------------------------------------------------------

/// Convert `value` expressed in `from` to `to`, both within `cat`.
///
/// Linear categories go through the base unit via each unit's factor.
/// [`Category::Temperature`] is handled specially (offset formulas). Converting
/// a unit to itself returns `value` unchanged (up to `f64` rounding — for
/// linear units it is bit-exact because the factors cancel).
///
/// The caller is responsible for passing `from`/`to` that actually belong to
/// `cat`; units from a different category would produce a meaningless number.
/// Using [`Category::units`] / [`Category::unit_by_id`] guarantees this.
pub fn convert(cat: Category, from: &Unit, to: &Unit, value: f64) -> f64 {
    if cat == Category::Temperature {
        return convert_temperature(from.id, to.id, value);
    }
    // Fast path: identical unit ⇒ exact identity, no float drift.
    if from.factor == to.factor {
        return value;
    }
    let value_in_base = value * from.factor;
    value_in_base / to.factor
}

/// Temperature conversion via Celsius as the pivot.
///
/// `F = C * 9/5 + 32`, `K = C + 273.15` (and their inverses). Unknown ids fall
/// back to treating the input as already-Celsius, which keeps the function
/// total; callers should only pass ids from [`TEMPERATURE`].
fn convert_temperature(from_id: &str, to_id: &str, value: f64) -> f64 {
    // Step 1: normalise the input to Celsius.
    let celsius = match from_id {
        "celsius" => value,
        "fahrenheit" => (value - 32.0) * 5.0 / 9.0,
        "kelvin" => value - 273.15,
        _ => value,
    };
    // Step 2: Celsius → target.
    match to_id {
        "celsius" => celsius,
        "fahrenheit" => celsius * 9.0 / 5.0 + 32.0,
        "kelvin" => celsius + 273.15,
        _ => celsius,
    }
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

/// Format a converted value for display.
///
/// Delegates to [`crate::engine::format_result`] so conversions share the
/// calculator's number rendering: thousands grouping, trailing-zero trimming,
/// ~12-significant-figure rounding, and scientific notation for extreme
/// magnitudes. Exposed as a named helper so the UI has a single, stable call
/// site and we can tune conversion precision here later without touching the UI.
pub fn format_conversion(value: f64) -> String {
    crate::engine::format_result(value)
}

/// True when a converted value is not a finite number (overflow / invalid).
///
/// A linear conversion multiplies by `from.factor` and divides by `to.factor`;
/// with extreme magnitudes and factors this can overflow `f64` to `inf`
/// (or produce `NaN`). The plain [`format_result`](crate::engine::format_result)
/// renderer maps those to a bare `"∞"`, which would be shown and copied as if it
/// were a real result. The UI uses this at the converter boundary to surface an
/// error state instead — mirroring how the main calculator reports overflow.
// wired by the converter UI overflow fix
#[allow(dead_code)]
pub fn is_overflow(value: f64) -> bool {
    !value.is_finite()
}

// ---------------------------------------------------------------------------
// Unit tables
// ---------------------------------------------------------------------------
//
// Conventions:
//   * The FIRST unit in every linear table is the base unit (factor == 1.0).
//   * `factor` = number of base units in ONE of this unit.
//   * Constants are exact where the definition is exact (e.g. 1 inch = 0.0254 m
//     exactly, 1 lb = 0.45359237 kg exactly) and best authoritative values
//     otherwise (documented inline).

/// Length — base unit: **meter**.
static LENGTH: &[Unit] = &[
    Unit { id: "meter",         name: "Meter",         symbol: "m",    factor: 1.0 },
    Unit { id: "kilometer",     name: "Kilometer",     symbol: "km",   factor: 1000.0 },
    Unit { id: "centimeter",    name: "Centimeter",    symbol: "cm",   factor: 0.01 },
    Unit { id: "millimeter",    name: "Millimeter",    symbol: "mm",   factor: 0.001 },
    Unit { id: "micrometer",    name: "Micrometer",    symbol: "\u{00B5}m", factor: 1e-6 },
    Unit { id: "mile",          name: "Mile",          symbol: "mi",   factor: 1609.344 },       // 1 mi = 1760 yd exactly
    Unit { id: "yard",          name: "Yard",          symbol: "yd",   factor: 0.9144 },         // exact
    Unit { id: "foot",          name: "Foot",          symbol: "ft",   factor: 0.3048 },         // exact
    Unit { id: "inch",          name: "Inch",          symbol: "in",   factor: 0.0254 },         // exact
    Unit { id: "nautical_mile", name: "Nautical mile", symbol: "nmi",  factor: 1852.0 },         // exact (international)
];

/// Area — base unit: **square meter**.
static AREA: &[Unit] = &[
    Unit { id: "square_meter",      name: "Square meter",      symbol: "m\u{00B2}",  factor: 1.0 },
    Unit { id: "square_kilometer",  name: "Square kilometer",  symbol: "km\u{00B2}", factor: 1_000_000.0 },
    Unit { id: "square_centimeter", name: "Square centimeter", symbol: "cm\u{00B2}", factor: 1e-4 },
    Unit { id: "square_millimeter", name: "Square millimeter", symbol: "mm\u{00B2}", factor: 1e-6 },
    Unit { id: "hectare",           name: "Hectare",           symbol: "ha",         factor: 10_000.0 },
    Unit { id: "acre",              name: "Acre",              symbol: "ac",          factor: 4046.8564224 }, // = 4840 yd² exactly
    Unit { id: "square_mile",       name: "Square mile",       symbol: "mi\u{00B2}", factor: 2_589_988.110336 }, // = 1609.344² exactly
    Unit { id: "square_yard",       name: "Square yard",       symbol: "yd\u{00B2}", factor: 0.83612736 },   // = 0.9144² exactly
    Unit { id: "square_foot",       name: "Square foot",       symbol: "ft\u{00B2}", factor: 0.09290304 },   // = 0.3048² exactly
    Unit { id: "square_inch",       name: "Square inch",       symbol: "in\u{00B2}", factor: 0.00064516 },   // = 0.0254² exactly
];

/// Volume — base unit: **liter**. All customary units are **US** measures.
static VOLUME: &[Unit] = &[
    Unit { id: "liter",           name: "Liter",            symbol: "L",     factor: 1.0 },
    Unit { id: "milliliter",      name: "Milliliter",       symbol: "mL",    factor: 0.001 },
    Unit { id: "cubic_meter",     name: "Cubic meter",      symbol: "m\u{00B3}",  factor: 1000.0 },
    Unit { id: "cubic_centimeter",name: "Cubic centimeter", symbol: "cm\u{00B3}", factor: 0.001 },
    // US customary liquid measures. 1 US gallon = 3.785411784 L (231 in³) exactly.
    Unit { id: "gallon_us",       name: "Gallon (US)",      symbol: "gal",   factor: 3.785411784 },
    Unit { id: "quart_us",        name: "Quart (US)",       symbol: "qt",    factor: 0.946352946 },      // gal / 4
    Unit { id: "pint_us",         name: "Pint (US)",        symbol: "pt",    factor: 0.473176473 },      // gal / 8
    Unit { id: "cup_us",          name: "Cup (US)",         symbol: "cup",   factor: 0.2365882365 },     // gal / 16 (US legal cup = 240 mL differs; this is the customary 1/16 gal)
    Unit { id: "fluid_ounce_us",  name: "Fluid ounce (US)", symbol: "fl oz", factor: 0.0295735295625 },  // gal / 128
    Unit { id: "tablespoon_us",   name: "Tablespoon (US)",  symbol: "tbsp",  factor: 0.01478676478125 }, // fl oz / 2
    Unit { id: "teaspoon_us",     name: "Teaspoon (US)",    symbol: "tsp",   factor: 0.00492892159375 }, // fl oz / 6
    Unit { id: "cubic_foot",      name: "Cubic foot",       symbol: "ft\u{00B3}", factor: 28.316846592 }, // = 0.3048³ m³ → L
    Unit { id: "cubic_inch",      name: "Cubic inch",       symbol: "in\u{00B3}", factor: 0.016387064 },  // = 0.0254³ m³ → L
];

/// Mass (Weight) — base unit: **kilogram**.
static MASS: &[Unit] = &[
    Unit { id: "kilogram",     name: "Kilogram",      symbol: "kg",  factor: 1.0 },
    Unit { id: "gram",         name: "Gram",          symbol: "g",   factor: 0.001 },
    Unit { id: "milligram",    name: "Milligram",     symbol: "mg",  factor: 1e-6 },
    Unit { id: "microgram",    name: "Microgram",     symbol: "\u{00B5}g", factor: 1e-9 },
    Unit { id: "metric_tonne", name: "Metric tonne",  symbol: "t",   factor: 1000.0 },
    Unit { id: "pound",        name: "Pound",         symbol: "lb",  factor: 0.45359237 },        // exact (international avoirdupois)
    Unit { id: "ounce",        name: "Ounce",         symbol: "oz",  factor: 0.028349523125 },    // lb / 16 exact
    Unit { id: "stone",        name: "Stone",         symbol: "st",  factor: 6.35029318 },        // 14 lb exact
    Unit { id: "us_ton",       name: "US ton",        symbol: "ton", factor: 907.18474 },         // short ton = 2000 lb exact
];

/// Temperature — SPECIAL (offset conversions, not factors). The `factor` field
/// is unused here; [`convert`] routes this category through [`convert_temperature`].
static TEMPERATURE: &[Unit] = &[
    Unit { id: "celsius",    name: "Celsius",    symbol: "\u{00B0}C", factor: 1.0 },
    Unit { id: "fahrenheit", name: "Fahrenheit", symbol: "\u{00B0}F", factor: 1.0 },
    Unit { id: "kelvin",     name: "Kelvin",     symbol: "K",         factor: 1.0 },
];

/// Speed — base unit: **meter per second**.
static SPEED: &[Unit] = &[
    Unit { id: "meter_per_second",    name: "Meter per second",    symbol: "m/s",  factor: 1.0 },
    Unit { id: "kilometer_per_hour",  name: "Kilometer per hour",  symbol: "km/h", factor: 1000.0 / 3600.0 },   // = 0.277… m/s
    Unit { id: "mile_per_hour",       name: "Mile per hour",       symbol: "mph",  factor: 1609.344 / 3600.0 }, // = 0.44704 m/s exact
    Unit { id: "foot_per_second",     name: "Foot per second",     symbol: "ft/s", factor: 0.3048 },            // exact
    Unit { id: "knot",                name: "Knot",                symbol: "kn",   factor: 1852.0 / 3600.0 },   // 1 nmi/h
];

/// Time — base unit: **second**.
///
/// `month` and `year` use conventional averages: a Gregorian year of 365.2425
/// days, and a month of that year / 12 (≈ 30.436875 days), NOT a flat 30-day
/// month. This is noted so the UI can surface the assumption.
static TIME: &[Unit] = &[
    Unit { id: "second",      name: "Second",      symbol: "s",   factor: 1.0 },
    Unit { id: "millisecond", name: "Millisecond", symbol: "ms",  factor: 0.001 },
    Unit { id: "microsecond", name: "Microsecond", symbol: "\u{00B5}s", factor: 1e-6 },
    Unit { id: "minute",      name: "Minute",      symbol: "min", factor: 60.0 },
    Unit { id: "hour",        name: "Hour",        symbol: "h",   factor: 3600.0 },
    Unit { id: "day",         name: "Day",         symbol: "d",   factor: 86_400.0 },
    Unit { id: "week",        name: "Week",        symbol: "wk",  factor: 604_800.0 },
    // Average Gregorian month = year / 12; average year = 365.2425 d.
    Unit { id: "month",       name: "Month (avg)", symbol: "mo",  factor: 2_629_746.0 },   // 365.2425/12 * 86400
    Unit { id: "year",        name: "Year (avg)",  symbol: "yr",  factor: 31_556_952.0 },  // 365.2425 * 86400
];

/// Data (digital storage) — base unit: **byte**.
///
/// Decimal (SI) prefixes are 1000-based; binary (IEC) prefixes are 1024-based.
/// Both families are provided explicitly so `1 KB = 1000 B` and `1 KiB = 1024 B`.
static DATA: &[Unit] = &[
    Unit { id: "byte",     name: "Byte",     symbol: "B",   factor: 1.0 },
    Unit { id: "bit",      name: "Bit",      symbol: "bit", factor: 0.125 },       // 1 byte = 8 bits
    // Decimal / SI (1000-based).
    Unit { id: "kilobyte", name: "Kilobyte", symbol: "KB",  factor: 1e3 },
    Unit { id: "megabyte", name: "Megabyte", symbol: "MB",  factor: 1e6 },
    Unit { id: "gigabyte", name: "Gigabyte", symbol: "GB",  factor: 1e9 },
    Unit { id: "terabyte", name: "Terabyte", symbol: "TB",  factor: 1e12 },
    Unit { id: "petabyte", name: "Petabyte", symbol: "PB",  factor: 1e15 },
    // Binary / IEC (1024-based).
    Unit { id: "kibibyte", name: "Kibibyte", symbol: "KiB", factor: 1024.0 },
    Unit { id: "mebibyte", name: "Mebibyte", symbol: "MiB", factor: 1_048_576.0 },        // 1024²
    Unit { id: "gibibyte", name: "Gibibyte", symbol: "GiB", factor: 1_073_741_824.0 },    // 1024³
    Unit { id: "tebibyte", name: "Tebibyte", symbol: "TiB", factor: 1_099_511_627_776.0 },// 1024⁴
];

/// Pressure — base unit: **pascal**.
static PRESSURE: &[Unit] = &[
    Unit { id: "pascal",     name: "Pascal",     symbol: "Pa",   factor: 1.0 },
    Unit { id: "kilopascal", name: "Kilopascal", symbol: "kPa",  factor: 1000.0 },
    Unit { id: "bar",        name: "Bar",        symbol: "bar",  factor: 100_000.0 },      // exact
    Unit { id: "psi",        name: "PSI",        symbol: "psi",  factor: 6894.757293168361 }, // lbf/in² (uses g₀, lb, inch — see note)
    Unit { id: "atmosphere", name: "Atmosphere", symbol: "atm",  factor: 101_325.0 },      // standard atm, exact by definition
    Unit { id: "torr",       name: "Torr (mmHg)",symbol: "Torr", factor: 133.32236842105263 }, // 101325 / 760 exact
];

/// Energy — base unit: **joule**.
static ENERGY: &[Unit] = &[
    Unit { id: "joule",         name: "Joule",         symbol: "J",   factor: 1.0 },
    Unit { id: "kilojoule",     name: "Kilojoule",     symbol: "kJ",  factor: 1000.0 },
    Unit { id: "calorie",       name: "Calorie",       symbol: "cal", factor: 4.184 },      // thermochemical calorie, exact
    Unit { id: "kilocalorie",   name: "Kilocalorie",   symbol: "kcal",factor: 4184.0 },     // food Calorie
    Unit { id: "watt_hour",     name: "Watt-hour",     symbol: "Wh",  factor: 3600.0 },
    Unit { id: "kilowatt_hour", name: "Kilowatt-hour", symbol: "kWh", factor: 3_600_000.0 },
    Unit { id: "btu",           name: "BTU",           symbol: "BTU", factor: 1055.05585262 }, // ISO/IT BTU
    Unit { id: "electronvolt",  name: "Electronvolt",  symbol: "eV",  factor: 1.602176634e-19 }, // exact (2019 SI)
    Unit { id: "foot_pound",    name: "Foot-pound",    symbol: "ft·lbf", factor: 1.3558179483314004 }, // lbf·ft
];

/// Power — base unit: **watt**.
static POWER: &[Unit] = &[
    Unit { id: "watt",           name: "Watt",           symbol: "W",     factor: 1.0 },
    Unit { id: "kilowatt",       name: "Kilowatt",       symbol: "kW",    factor: 1000.0 },
    Unit { id: "megawatt",       name: "Megawatt",       symbol: "MW",    factor: 1_000_000.0 },
    Unit { id: "horsepower",     name: "Horsepower",     symbol: "hp",    factor: 745.6998715822702 }, // mechanical/imperial hp = 550 ft·lbf/s
    Unit { id: "btu_per_hour",   name: "BTU/hour",       symbol: "BTU/h", factor: 1055.05585262 / 3600.0 },
    Unit { id: "foot_pound_per_second", name: "Foot-pound/second", symbol: "ft·lbf/s", factor: 1.3558179483314004 },
];

/// Angle — base unit: **degree**.
static ANGLE: &[Unit] = &[
    Unit { id: "degree",     name: "Degree",     symbol: "\u{00B0}",  factor: 1.0 },
    Unit { id: "radian",     name: "Radian",     symbol: "rad",       factor: 180.0 / std::f64::consts::PI },
    Unit { id: "gradian",    name: "Gradian",    symbol: "grad",      factor: 0.9 },        // 400 grad = 360°
    Unit { id: "arcminute",  name: "Arcminute",  symbol: "\u{2032}",  factor: 1.0 / 60.0 },
    Unit { id: "arcsecond",  name: "Arcsecond",  symbol: "\u{2033}",  factor: 1.0 / 3600.0 },
    Unit { id: "revolution", name: "Revolution", symbol: "rev",       factor: 360.0 },
];

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Relative-tolerance comparison (~1e-6). Falls back to absolute tolerance
    /// near zero, where relative error is undefined.
    fn approx(actual: f64, expected: f64) -> bool {
        let diff = (actual - expected).abs();
        if expected.abs() < 1e-12 {
            diff < 1e-9
        } else {
            diff / expected.abs() < 1e-6
        }
    }

    /// Convenience: convert by ids within a category (panics if an id is wrong,
    /// which is exactly what we want in tests — it catches typos in the tables).
    fn conv(cat: Category, from: &str, to: &str, value: f64) -> f64 {
        let f = cat.unit_by_id(from).unwrap_or_else(|| panic!("no unit {from} in {}", cat.name()));
        let t = cat.unit_by_id(to).unwrap_or_else(|| panic!("no unit {to} in {}", cat.name()));
        convert(cat, f, t, value)
    }

    // -- Length -------------------------------------------------------------

    #[test]
    fn length_golden() {
        assert!(approx(conv(Category::Length, "kilometer", "meter", 1.0), 1000.0));
        assert!(approx(conv(Category::Length, "mile", "meter", 1.0), 1609.344));
        assert!(approx(conv(Category::Length, "mile", "kilometer", 1.0), 1.609344));
        assert!(approx(conv(Category::Length, "inch", "centimeter", 1.0), 2.54));
        assert!(approx(conv(Category::Length, "foot", "meter", 1.0), 0.3048));
        assert!(approx(conv(Category::Length, "nautical_mile", "meter", 1.0), 1852.0));
    }

    // -- Mass ---------------------------------------------------------------

    #[test]
    fn mass_golden() {
        assert!(approx(conv(Category::Mass, "kilogram", "gram", 1.0), 1000.0));
        assert!(approx(conv(Category::Mass, "pound", "kilogram", 1.0), 0.45359237));
        assert!(approx(conv(Category::Mass, "kilogram", "pound", 1.0), 2.2046226218487757));
        assert!(approx(conv(Category::Mass, "ounce", "gram", 1.0), 28.349523125));
        assert!(approx(conv(Category::Mass, "stone", "pound", 1.0), 14.0));
    }

    // -- Temperature (special) ---------------------------------------------

    #[test]
    fn temperature_golden() {
        assert!(approx(conv(Category::Temperature, "celsius", "fahrenheit", 100.0), 212.0));
        assert!(approx(conv(Category::Temperature, "celsius", "fahrenheit", 0.0), 32.0));
        assert!(approx(conv(Category::Temperature, "celsius", "kelvin", 100.0), 373.15));
        assert!(approx(conv(Category::Temperature, "celsius", "fahrenheit", -40.0), -40.0));
        assert!(approx(conv(Category::Temperature, "fahrenheit", "celsius", 32.0), 0.0));
        assert!(approx(conv(Category::Temperature, "kelvin", "celsius", 373.15), 100.0));
    }

    #[test]
    fn temperature_roundtrip() {
        for c in [-273.15, -40.0, 0.0, 37.0, 100.0, 1234.5] {
            let f = conv(Category::Temperature, "celsius", "fahrenheit", c);
            let back = conv(Category::Temperature, "fahrenheit", "celsius", f);
            assert!(approx(back, c), "C→F→C failed for {c}: got {back}");
            let k = conv(Category::Temperature, "celsius", "kelvin", c);
            let back_k = conv(Category::Temperature, "kelvin", "celsius", k);
            assert!(approx(back_k, c), "C→K→C failed for {c}: got {back_k}");
        }
    }

    // -- Speed --------------------------------------------------------------

    #[test]
    fn speed_golden() {
        assert!(approx(conv(Category::Speed, "meter_per_second", "kilometer_per_hour", 1.0), 3.6));
        assert!(approx(conv(Category::Speed, "mile_per_hour", "kilometer_per_hour", 60.0), 96.56064));
        assert!(approx(conv(Category::Speed, "knot", "kilometer_per_hour", 1.0), 1.852));
    }

    // -- Data ---------------------------------------------------------------

    #[test]
    fn data_golden() {
        assert!(approx(conv(Category::Data, "kilobyte", "byte", 1.0), 1000.0));
        assert!(approx(conv(Category::Data, "kibibyte", "byte", 1.0), 1024.0));
        assert!(approx(conv(Category::Data, "megabyte", "byte", 1.0), 1e6));
        assert!(approx(conv(Category::Data, "gibibyte", "byte", 1.0), 1_073_741_824.0));
        assert!(approx(conv(Category::Data, "byte", "bit", 1.0), 8.0));
    }

    // -- Pressure -----------------------------------------------------------

    #[test]
    fn pressure_golden() {
        assert!(approx(conv(Category::Pressure, "bar", "pascal", 1.0), 100_000.0));
        assert!(approx(conv(Category::Pressure, "atmosphere", "pascal", 1.0), 101_325.0));
        assert!(approx(conv(Category::Pressure, "psi", "pascal", 1.0), 6894.757));
    }

    // -- Volume -------------------------------------------------------------

    #[test]
    fn volume_golden() {
        assert!(approx(conv(Category::Volume, "gallon_us", "liter", 1.0), 3.785411784));
        assert!(approx(conv(Category::Volume, "liter", "milliliter", 1.0), 1000.0));
        assert!(approx(conv(Category::Volume, "cup_us", "milliliter", 1.0), 236.588), "cup≈236.588 mL");
    }

    // -- Energy -------------------------------------------------------------

    #[test]
    fn energy_golden() {
        assert!(approx(conv(Category::Energy, "kilocalorie", "joule", 1.0), 4184.0));
        assert!(approx(conv(Category::Energy, "kilowatt_hour", "joule", 1.0), 3.6e6));
        // BTU uses the ISO/IT definition, 1055.05585262 J. The spec's "≈1055.06"
        // is a 6-sig-fig rounding, so compare against the exact constant (which
        // rounds to 1055.06) rather than the truncated literal at 1e-6 rel. tol.
        assert!(approx(conv(Category::Energy, "btu", "joule", 1.0), 1055.05585262), "BTU (IT)≈1055.056 J");
    }

    // -- Angle --------------------------------------------------------------

    #[test]
    fn angle_golden() {
        assert!(approx(conv(Category::Angle, "degree", "radian", 180.0), std::f64::consts::PI));
        assert!(approx(conv(Category::Angle, "revolution", "degree", 1.0), 360.0));
        assert!(approx(conv(Category::Angle, "gradian", "degree", 1.0), 0.9));
    }

    // -- Area ---------------------------------------------------------------

    #[test]
    fn area_golden() {
        assert!(approx(conv(Category::Area, "hectare", "square_meter", 1.0), 10_000.0));
        assert!(approx(conv(Category::Area, "acre", "square_meter", 1.0), 4046.856), "acre≈4046.856 m²");
    }

    // -- Time ---------------------------------------------------------------

    #[test]
    fn time_golden() {
        assert!(approx(conv(Category::Time, "hour", "second", 1.0), 3600.0));
        assert!(approx(conv(Category::Time, "day", "second", 1.0), 86_400.0));
    }

    // -- Power --------------------------------------------------------------

    #[test]
    fn power_golden() {
        assert!(approx(conv(Category::Power, "horsepower", "watt", 1.0), 745.6999), "hp≈745.7 W");
        assert!(approx(conv(Category::Power, "kilowatt", "watt", 1.0), 1000.0));
    }

    // -- Self-conversion & identity ----------------------------------------

    #[test]
    fn self_conversion_is_identity() {
        let m = Category::Length.unit_by_id("meter").unwrap();
        assert_eq!(convert(Category::Length, m, m, 5.0), 5.0);
        // Non-base linear self-conversion is also exact via the fast path.
        let mi = Category::Length.unit_by_id("mile").unwrap();
        assert_eq!(convert(Category::Length, mi, mi, 42.0), 42.0);
        // Temperature self-conversion.
        let c = Category::Temperature.unit_by_id("celsius").unwrap();
        assert!(approx(convert(Category::Temperature, c, c, 21.0), 21.0));
    }

    // -- Table / API invariants --------------------------------------------

    #[test]
    fn all_categories_have_units_and_valid_defaults() {
        for &cat in Category::all() {
            let units = cat.units();
            assert!(units.len() >= 2, "{} has too few units", cat.name());
            // default_from / default_to must be real members of the category.
            let from_id = cat.default_from().id;
            let to_id = cat.default_to().id;
            assert!(cat.unit_by_id(from_id).is_some(), "{} default_from bad", cat.name());
            assert!(cat.unit_by_id(to_id).is_some(), "{} default_to bad", cat.name());
            assert_ne!(from_id, to_id, "{} defaults should differ", cat.name());
        }
    }

    #[test]
    fn linear_categories_have_a_unit_base_first() {
        for &cat in Category::all() {
            if cat == Category::Temperature {
                continue; // temperature factors are placeholders
            }
            assert_eq!(
                cat.units()[0].factor,
                1.0,
                "{} first unit must be the base (factor 1.0)",
                cat.name()
            );
        }
    }

    #[test]
    fn unit_ids_are_unique_within_category() {
        for &cat in Category::all() {
            let units = cat.units();
            for (i, a) in units.iter().enumerate() {
                for b in &units[i + 1..] {
                    assert_ne!(a.id, b.id, "duplicate id {} in {}", a.id, cat.name());
                }
            }
        }
    }

    #[test]
    fn unit_by_id_unknown_is_none() {
        assert!(Category::Length.unit_by_id("furlong").is_none());
    }

    #[test]
    fn all_has_every_category_once() {
        assert_eq!(Category::all().len(), 12);
    }

    // -- Formatting reuse ---------------------------------------------------

    #[test]
    fn format_conversion_matches_engine() {
        // Delegates to engine::format_result: grouping + trailing-zero trim.
        assert_eq!(format_conversion(1000.0), "1,000");
        assert_eq!(format_conversion(2.5000), "2.5");
        assert_eq!(format_conversion(conv(Category::Length, "kilometer", "meter", 5.0)), "5,000");
    }

    // -- Overflow / non-finite guard ---------------------------------------

    #[test]
    fn overflow_conversion_detected() {
        // mile (factor 1609.344) → micrometer (factor 1e-6) multiplies by ~1.6e9,
        // so a near-f64::MAX input overflows to a non-finite result.
        let result = conv(Category::Length, "mile", "micrometer", 1e308);
        assert!(!result.is_finite(), "expected overflow to inf, got {result}");
        assert!(is_overflow(result), "is_overflow should flag the overflow");
    }

    #[test]
    fn normal_conversion_not_overflow() {
        // A run-of-the-mill conversion stays finite.
        let result = conv(Category::Length, "kilometer", "meter", 1.0);
        assert!(approx(result, 1000.0));
        assert!(!is_overflow(result), "normal result must not be flagged");
    }
}
