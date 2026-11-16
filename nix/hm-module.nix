# home-manager module: `services.keytle`.
#
# Same options as the NixOS module, but per user: the package goes into
# `home.packages` and the settings into `xdg.configFile`, which is exactly where
# keytle looks on its own ($XDG_CONFIG_HOME/keytle/config.toml).
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
        Settings rendered to {file}`$XDG_CONFIG_HOME/keytle/config.toml` with
        {var}`pkgs.formats.toml`. Every key is optional; keytle rejects unknown
        keys, so a typo fails loudly instead of being ignored.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];

    xdg.configFile."keytle/config.toml".source = toml.generate "keytle-config.toml" cfg.settings;
  };
}
