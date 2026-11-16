# NixOS module: `services.keytle`.
#
# keytle finds its config directory as $KEYTLE_CONFIG_DIR, then
# $XDG_CONFIG_HOME/keytle, then $HOME/.config/keytle. A system-wide config
# therefore goes to /etc/keytle/config.toml with KEYTLE_CONFIG_DIR pointing at
# it; test history and profiles stay per user under $XDG_DATA_HOME/keytle, so
# nobody writes into /etc at runtime.
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.keytle;
  toml = pkgs.formats.toml { };
in
{
  options.services.keytle = {
    enable = lib.mkEnableOption "keytle, an offline TUI that teaches keyboard layouts";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./package.nix { };
      defaultText = lib.literalExpression "pkgs.callPackage ./package.nix { }";
      description = "The keytle package to install.";
    };

    settings = lib.mkOption {
      type = toml.type;
      default = { };
      example = lib.literalExpression ''
        {
          layout = "colemak-dh";
          language = "fr";
          target_wpm = 90.0;
        }
      '';
      description = ''
        Settings rendered to {file}`/etc/keytle/config.toml` with
        {var}`pkgs.formats.toml`. Every key is optional; keytle rejects unknown
        keys, so a typo fails loudly instead of being ignored.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];

    environment.etc."keytle/config.toml".source = toml.generate "keytle-config.toml" cfg.settings;

    environment.sessionVariables.KEYTLE_CONFIG_DIR = "/etc/keytle";
  };
}
