flake:

{ config, lib, pkgs, ... }:

let
  cfg = config.services.qssh;
  settingsFormat = pkgs.formats.toml { };
  configFile = settingsFormat.generate "qssh.toml" cfg.settings;
in
{
  options.services.qssh = {
    enable = lib.mkEnableOption "qssh QUIC SSH proxy server";

    package = lib.mkPackageOption pkgs "qssh" {
      default = flake.packages.${pkgs.system}.qssh;
    };

    logLevel = lib.mkOption {
      type = lib.types.enum [ "trace" "debug" "info" "warn" "error" ];
      default = "info";
      description = "Log level for the qssh server.";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 4433;
      description = "UDP port for the firewall rule. Should match the port in `settings.server.listen`.";
    };

    settings = lib.mkOption {
      type = lib.types.submodule {
        freeformType = settingsFormat.type;

        options.server = {
          listen = lib.mkOption {
            type = lib.types.str;
            default = "0.0.0.0:4433";
            description = "Address to listen on (host:port).";
          };

          proxy_to = lib.mkOption {
            type = lib.types.str;
            default = "127.0.0.1:22";
            description = "Default upstream SSH server address.";
          };

          cert_dir = lib.mkOption {
            type = lib.types.str;
            default = "/var/lib/qssh";
            description = "Directory for auto-generated TLS certificates.";
          };
        };
      };
      default = { };
      description = "qssh server configuration (TOML).";
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Whether to open the UDP port in the firewall.";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.services.qssh = {
      description = "qssh QUIC SSH proxy";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" "sshd.service" ];

      serviceConfig = {
        ExecStart = "${lib.getExe cfg.package} --log-level ${cfg.logLevel} server -c ${configFile}";
        Restart = "on-failure";
        RestartSec = 5;

        # Hardening
        DynamicUser = true;
        StateDirectory = "qssh";
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        ReadWritePaths = [ "/var/lib/qssh" ];
      };
    };

    networking.firewall.allowedUDPPorts =
      lib.mkIf cfg.openFirewall [ cfg.port ];
  };
}
