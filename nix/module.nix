flake:

{ config, lib, pkgs, ... }:

let
  cfg = config.services.quicssh;
  settingsFormat = pkgs.formats.toml { };
  configFile = settingsFormat.generate "quicssh.toml" cfg.settings;
in
{
  options.services.quicssh = {
    enable = lib.mkEnableOption "quicssh QUIC SSH proxy server";

    package = lib.mkPackageOption pkgs "quicssh" {
      default = flake.packages.${pkgs.system}.quicssh;
    };

    logLevel = lib.mkOption {
      type = lib.types.enum [ "trace" "debug" "info" "warn" "error" ];
      default = "info";
      description = "Log level for the quicssh server.";
    };

    settings = lib.mkOption {
      type = lib.types.submodule {
        freeformType = settingsFormat.type;

        options.server = {
          listen = lib.mkOption {
            type = lib.types.str;
            default = "0.0.0.0:4433";
            description = "Address to listen on.";
          };

          proxy_to = lib.mkOption {
            type = lib.types.str;
            default = "127.0.0.1:22";
            description = "Default upstream SSH server address.";
          };

          cert_dir = lib.mkOption {
            type = lib.types.str;
            default = "/var/lib/quicssh";
            description = "Directory for auto-generated TLS certificates.";
          };
        };
      };
      default = { };
      description = "quicssh server configuration (TOML).";
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Whether to open the UDP port in the firewall.";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.services.quicssh = {
      description = "quicssh QUIC SSH proxy";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" "sshd.service" ];

      serviceConfig = {
        ExecStart = "${lib.getExe cfg.package} --log-level ${cfg.logLevel} server -c ${configFile}";
        Restart = "on-failure";
        RestartSec = 5;

        # Hardening
        DynamicUser = true;
        StateDirectory = "quicssh";
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        ReadWritePaths = [ "/var/lib/quicssh" ];
      };
    };

    networking.firewall.allowedUDPPorts =
      let
        port = lib.toInt (lib.last (lib.splitString ":" cfg.settings.server.listen));
      in
      lib.mkIf cfg.openFirewall [ port ];
  };
}
