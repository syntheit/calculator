{
  description = "Calculator — a native GTK4/libadwaita calculator (Google Calculator-style, mobile-first)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      fenix,
      crane,
    }:
    # x86_64-linux for dev, aarch64-linux for the real target (OnePlus 6T /
    # GNOME Shell Mobile). Both use the same crane+fenix pipeline.
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (
      system:
      let
        pkgs = import nixpkgs { inherit system; };

        # Pinned stable Rust toolchain via fenix (reproducible, works on aarch64 too).
        rustToolchain = fenix.packages.${system}.stable.toolchain;

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        # Native build inputs needed at compile time.
        nativeBuildInputs = with pkgs; [
          pkg-config
          wrapGAppsHook4
          glib # provides glib-compile-schemas
          desktop-file-utils
          appstream # validate metainfo
        ];

        # Libraries the app links against. The gtk4-rs -sys crates link these
        # directly, so they must be present at link time AND on the runtime
        # library path (see preFixup / shellHook).
        buildInputs = with pkgs; [
          glib
          gtk4
          libadwaita
          pango
          cairo
          gdk-pixbuf
          graphene
          harfbuzz
        ];

        # Cleaned source (Rust/TOML only) for the dependency layer — keeps the
        # crane cache warm across data/README/etc. edits.
        cleanSrc = craneLib.cleanCargoSource ./.;

        # Full source for the final build so postInstall can reach data/*.
        fullSrc = pkgs.lib.fileset.toSource {
          root = ./.;
          fileset = pkgs.lib.fileset.unions [
            (craneLib.fileset.commonCargoSources ./.)
            ./data
          ];
        };

        commonArgs = {
          inherit nativeBuildInputs buildInputs;
          strictDeps = true;
        };

        # Dependencies compiled against the cleaned source.
        cargoArtifacts = craneLib.buildDepsOnly (commonArgs // { src = cleanSrc; });

        calculator = craneLib.buildPackage (
          commonArgs
          // {
            src = fullSrc;
            inherit cargoArtifacts;

            # Install the desktop file, icon, metainfo and gschema, then compile
            # the schema so the installed app launches without GSETTINGS_SCHEMA_DIR.
            postInstall = ''
              install -Dm644 data/io.matv.Calculator.desktop \
                $out/share/applications/io.matv.Calculator.desktop
              install -Dm644 data/io.matv.Calculator.metainfo.xml \
                $out/share/metainfo/io.matv.Calculator.metainfo.xml
              install -Dm644 data/icons/hicolor/scalable/apps/io.matv.Calculator.svg \
                $out/share/icons/hicolor/scalable/apps/io.matv.Calculator.svg
              install -Dm644 data/io.matv.Calculator.gschema.xml \
                $out/share/glib-2.0/schemas/io.matv.Calculator.gschema.xml
              glib-compile-schemas $out/share/glib-2.0/schemas
            '';

            # crane doesn't stamp the GUI libraries into the binary's RPATH, so
            # put them on the wrapper's LD_LIBRARY_PATH. wrapGAppsHook4 applies
            # these gappsWrapperArgs to the executable(s) in $out/bin.
            preFixup = ''
              gappsWrapperArgs+=(
                --prefix LD_LIBRARY_PATH : "${pkgs.lib.makeLibraryPath buildInputs}"
              )
            '';

            meta = with pkgs.lib; {
              description = "Native GTK4/libadwaita calculator (Google Calculator-style, mobile-first)";
              homepage = "https://github.com/syntheit/calculator";
              license = licenses.gpl3Plus;
              mainProgram = "calculator";
              platforms = [ "x86_64-linux" "aarch64-linux" ];
            };
          }
        );
      in
      {
        packages = {
          default = calculator;
          calculator = calculator;
        };

        apps.default = flake-utils.lib.mkApp {
          drv = calculator;
          name = "calculator";
        };

        devShells.default = pkgs.mkShell {
          inherit buildInputs;
          nativeBuildInputs = nativeBuildInputs ++ [
            rustToolchain
            fenix.packages.${system}.stable.rust-analyzer
            pkgs.clippy
          ];

          # Point gio::Settings at the locally compiled schema during dev, and
          # make the GUI libs discoverable for the unwrapped `cargo run` binary.
          shellHook = ''
            export GSETTINGS_SCHEMA_DIR="$PWD/data"
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath buildInputs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
            if [ -f data/io.matv.Calculator.gschema.xml ]; then
              glib-compile-schemas data 2>/dev/null || true
            fi
            echo "calculator devshell — run: cargo run"
          '';
        };
      }
    );
}
