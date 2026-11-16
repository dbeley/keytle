# The keytle derivation, shared by the flake, the overlay and both modules.
#
# There are no runtime data dependencies: the corpora and layouts are baked in
# with `include_str!`, so `$out` is just the binary. The workspace's root
# manifest is virtual, which `cargo build` and `cargo test` handle by acting on
# every member, so no `buildAndTestSubdir` is needed.
{
  lib,
  rustPlatform,
}:

rustPlatform.buildRustPackage {
  pname = "keytle";
  version = (lib.importTOML ../Cargo.toml).workspace.package.version;

  src = lib.cleanSource ../.;

  cargoLock.lockFile = ../Cargo.lock;

  # Runs the whole workspace (keytle-core's unit tests plus keytle's CLI tests).
  cargoTestFlags = [ "--workspace" ];

  meta = {
    description = "A fast, offline TUI that teaches keyboard layouts";
    license = with lib.licenses; [
      mit
      asl20
    ];
    mainProgram = "keytle";
    platforms = lib.platforms.unix;
  };
}
