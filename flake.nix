{
  description = "keytle — a fast, offline TUI that teaches keyboard layouts";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;

      # Everything a single system exposes, derived once from its nixpkgs.
      perSystem =
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          keytle = pkgs.callPackage ./nix/package.nix { };

          # `cargo fmt` only needs rustfmt; the source copy is made writable so
          # cargo can not trip over the read-only store path.
          fmt =
            pkgs.runCommandLocal "keytle-fmt-check"
              {
                nativeBuildInputs = [
                  pkgs.cargo
                  pkgs.rustfmt
                ];
              }
              ''
                cp -r ${self} source
                chmod -R u+w source
                cd source
                export HOME=$TMPDIR
                cargo fmt --all --check
                touch $out
              '';
        in
        {
          packages = {
            default = keytle;
            inherit keytle;
          };

          checks = {
            inherit fmt;

            # Reuses the package's vendored dependencies; the phases are replaced
            # so the run is exactly the lint, with warnings promoted to errors.
            clippy = keytle.overrideAttrs (old: {
              pname = "${old.pname}-clippy";
              nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ [ pkgs.clippy ];
              buildPhase = ''
                runHook preBuild
                cargo clippy --offline --workspace --all-targets -- -D warnings
                runHook postBuild
              '';
              doCheck = false;
              installPhase = ''
                mkdir -p $out
                echo "cargo clippy --workspace --all-targets -- -D warnings passed" > $out/result
              '';
            });

            # `cargo test --workspace` in the dev profile (the release profile
            # sets `panic = "abort"`, which test binaries cannot use).
            test = keytle.overrideAttrs (old: {
              pname = "${old.pname}-test";
              buildPhase = ''
                runHook preBuild
                cargo test --offline --workspace
                runHook postBuild
              '';
              doCheck = false;
              installPhase = ''
                mkdir -p $out
                echo "cargo test --workspace passed" > $out/result
              '';
            });
          };

          devShells.default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              clippy
              rust-analyzer
              rustc
              rustfmt
            ];
          };

          apps.default = {
            type = "app";
            program = "${keytle}/bin/keytle";
            meta.description = "Run keytle";
          };

          formatter = pkgs.nixfmt-rfc-style;
        };
    in
    {
      packages = forAllSystems (system: (perSystem system).packages);
      checks = forAllSystems (system: (perSystem system).checks);
      formatter = forAllSystems (system: (perSystem system).formatter);

      devShells = forAllSystems (system: {
        default = (perSystem system).devShells.default;
      });

      apps = forAllSystems (system: {
        default = (perSystem system).apps.default;
        keytle = (perSystem system).apps.default;
      });

      overlays.default = final: _prev: {
        keytle = final.callPackage ./nix/package.nix { };
      };

      nixosModules.default = import ./nix/module.nix;
      homeModules.default = import ./nix/hm-module.nix;
    };
}
