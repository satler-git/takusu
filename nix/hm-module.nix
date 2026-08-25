{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.takusu-desktop;
  tomlFormat = pkgs.formats.toml { };
  hasSettings = cfg.settings != null;
  # The dedicated `storage` and `dataDir` options are authoritative: they are
  # always written into the generated TOML on top of any user-supplied
  # `settings` so the file stays consistent with the rest of the module.
  settingsToml =
    (if hasSettings then cfg.settings else { })
    // {
      storage = cfg.storage;
    }
    // lib.optionalAttrs (cfg.storage == "sqlite") {
      db = "sqlite:${cfg.dataDir}/takusu.db";
    };
in
{
  options.services.takusu-desktop = {
    enable = lib.mkEnableOption "the takusu resident desktop daemon";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.system}.takusu-desktop;
      defaultText = lib.literalExpression "self.packages.\${pkgs.system}.takusu-desktop";
      description = "The takusu-desktop package to use.";
    };

    dataDir = lib.mkOption {
      type = lib.types.str;
      default = "${config.home.homeDirectory}/.local/state/takusu";
      description = ''
        Directory used for the SQLite database and other runtime state.
        This path is also added to the service's `ReadWritePaths` so the
        daemon can write to it under `ProtectHome=read-only`.
      '';
    };

    settings = lib.mkOption {
      type = lib.types.nullOr (
        lib.types.submodule {
          freeformType = with lib.types; attrsOf anything;
        }
      );
      default = null;
      description = ''
        Contents of `~/.config/takusu/config.toml`. Use this for non-secret
        settings such as `desktop.theme` or `desktop.local_url`. When `null`,
        no config file is generated and the service relies on environment
        variables.

        Note that `storage` and `db` are always taken from the dedicated
        `storage` and `dataDir` options; any `storage` or `db` keys placed
        here are ignored.

        Secrets should not be placed here; use `tokenFile` and
        `jwtSecretFile` instead.
      '';
    };

    tokenFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = ''
        Path to a text file containing the root bearer token for the local
        API. The file content is passed to the daemon via
        `TAKUSU_TOKEN_FILE`.
      '';
    };

    jwtSecretFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = ''
        Path to a text file containing the JWT secret for the SQLite
        backend. The file content is passed to the daemon via
        `TAKUSU_JWT_SECRET_FILE`.
      '';
    };

    storage = lib.mkOption {
      type = lib.types.enum [
        "sqlite"
        "workers"
      ];
      default = "sqlite";
      description = ''
        Storage backend. `sqlite` uses a local SQLite database in `dataDir`;
        `workers` talks to a Cloudflare Worker backend (requires additional
        `TAKUSU_*` environment variables).
      '';
    };

    extraEnvironment = lib.mkOption {
      type = with lib.types; attrsOf str;
      default = { };
      description = ''
        Additional environment variables passed to the service. Use this for
        worker credentials or other non-default settings. Prefer the
        dedicated `*_FILE` options for secrets; values set here are stored in
        the Nix store through the generated systemd unit.
      '';
    };

    logLevel = lib.mkOption {
      type = lib.types.str;
      default = "info,takusu_desktop=debug";
      description = "Value of the `RUST_LOG` environment variable.";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion =
          cfg.storage == "workers"
          || cfg.jwtSecretFile != null
          || cfg.extraEnvironment ? "TAKUSU_JWT_SECRET"
          || cfg.extraEnvironment ? "TAKUSU_JWT_SECRET_FILE";
        message = "services.takusu-desktop.jwtSecretFile (or TAKUSU_JWT_SECRET/TAKUSU_JWT_SECRET_FILE in extraEnvironment) is required for the sqlite backend";
      }
      {
        assertion = cfg.storage == "sqlite" || cfg.extraEnvironment ? "TAKUSU_WORKERS_URL";
        message = "TAKUSU_WORKERS_URL must be set in extraEnvironment when using the workers backend";
      }
    ];

    systemd.user.services.takusu-desktop = {
      Unit = {
        Description = "takusu resident desktop daemon";
        After = [ "graphical-session.target" ];
      };

      Service = {
        Type = "simple";
        ExecStart = lib.getExe cfg.package;
        Restart = "on-failure";

        Environment = lib.mapAttrsToList (name: value: "${name}=${toString value}") (
          {
            RUST_LOG = cfg.logLevel;
          }
          // lib.optionalAttrs (!hasSettings) {
            TAKUSU_STORAGE = cfg.storage;
          }
          // lib.optionalAttrs (!hasSettings && cfg.storage == "sqlite") {
            TAKUSU_DB = "sqlite:${cfg.dataDir}/takusu.db";
          }
          // lib.optionalAttrs (cfg.tokenFile != null) { TAKUSU_TOKEN_FILE = toString cfg.tokenFile; }
          // lib.optionalAttrs (cfg.jwtSecretFile != null) {
            TAKUSU_JWT_SECRET_FILE = toString cfg.jwtSecretFile;
          }
          // cfg.extraEnvironment
        );

        StateDirectory = "takusu";
        NoNewPrivileges = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        ProtectKernelLogs = true;
        ProtectClock = true;
        ProtectHostname = true;
        ProtectHome = "read-only";
        ProtectSystem = "strict";
        ReadWritePaths = [
          cfg.dataDir
          "%t"
        ];
        PrivateTmp = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        RestrictAddressFamilies = [
          "AF_INET"
          "AF_INET6"
          "AF_UNIX"
        ];
        RestrictNamespaces = true;
        LockPersonality = true;
      };

      Install = {
        WantedBy = [ "default.target" ];
      };
    };

    home.file.".config/takusu/config.toml" = lib.mkIf hasSettings {
      source = tomlFormat.generate "takusu-config.toml" settingsToml;
    };
  };
}
