{
  description = "A full Rust flake";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    crane = {
      url = "github:ipetkov/crane";
    };

    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };

    mcp-servers-nix = {
      url = "github:natsukium/mcp-servers-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    treefmt-nix.url = "github:numtide/treefmt-nix";
    systems.url = "github:nix-systems/default";
  };

  outputs =
    inputs@{
      self,
      nixpkgs,
      flake-parts,
      crane,
      ...
    }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = import inputs.systems;

      imports = [
        inputs.treefmt-nix.flakeModule
      ];

      perSystem =
        {
          config,
          system,
          pkgs,
          lib,
          ...
        }:
        let
          rust-bin = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          craneLib = (crane.mkLib pkgs).overrideToolchain rust-bin;

          androidComposition = (pkgs.androidenv.composeAndroidPackages.override { licenseAccepted = true; }) {
            includeNDK = true;
            includeEmulator = false;
            includeSystemImages = false;
            includeSources = false;
            buildToolsVersions = [
              "37.0.0"
              "35.0.0"
            ];
            platformVersions = [
              "37"
              "35"
            ];
            cmakeVersions = [
              "3.22.1"
              "3.30.5"
            ];
          };
          androidNdkHome = "${androidComposition.ndk-bundle}/libexec/android-sdk/ndk-bundle";

          # Android SDK with emulator + x86_64 system image for local development.
          # Kept separate from androidComposition so the build/NDK closure is not
          # bloated by the emulator and system image downloads.
          emulatorComposition =
            (pkgs.androidenv.composeAndroidPackages.override { licenseAccepted = true; })
              {
                includeNDK = false;
                includeEmulator = true;
                includeSystemImages = true;
                includeSources = false;
                platformVersions = [ "35" ];
                systemImageTypes = [ "google_apis" ];
                abiVersions = [ "x86_64" ];
              };
          emulatorSdk = emulatorComposition.androidsdk;

          # Sherpa-ONNX prebuilt shared libraries for host tests and the Android
          # cross-build. Nix-managed so CI doesn't depend on the build.rs
          # download/extract cache under target/.
          sherpaOnnxAndroid =
            pkgs.runCommand "sherpa-onnx-android-1.13.4"
              {
                nativeBuildInputs = [
                  pkgs.bzip2
                  pkgs.gnutar
                ];
              }
              ''
                mkdir -p $out
                tar -xjf ${
                  pkgs.fetchurl {
                    url = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.4/sherpa-onnx-v1.13.4-android.tar.bz2";
                    hash = "sha256-eYP8PeI/bmQUjy+wX6lKLvqowFFswVczg9xcfU0qQ7A=";
                  }
                } -C $out
              '';

          # Newer Android NDKs (r23+) removed libgcc in favor of libunwind, but
          # tikv-jemalloc-sys still emits -lgcc for Android. Provide a libgcc.a
          # linker script that resolves to libunwind so jemalloc can still link.
          androidLibgccShim = pkgs.runCommand "android-libgcc-shim" { } ''
            mkdir -p $out/lib
            # libgcc.a as a linker script redirects -lgcc to -lunwind.
            echo 'INPUT(-lunwind)' > $out/lib/libgcc.a
          '';

          sherpaOnnxLinuxX64Shared =
            pkgs.runCommand "sherpa-onnx-linux-x64-shared-1.13.4"
              {
                nativeBuildInputs = [
                  pkgs.bzip2
                  pkgs.gnutar
                ];
              }
              ''
                mkdir -p $out
                tar -xjf ${
                  pkgs.fetchurl {
                    url = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.4/sherpa-onnx-v1.13.4-linux-x64-shared-lib.tar.bz2";
                    hash = "sha256-PnzoA3nJOGaPERV7H1SgJytAly9hj0RdyucdEidk0fo=";
                  }
                } -C $out
                mv $out/sherpa-onnx-v1.13.4-linux-x64-shared-lib/lib $out/lib
                rm -rf $out/sherpa-onnx-v1.13.4-linux-x64-shared-lib
                chmod -R +w $out/lib
              '';

          rootSrc = lib.cleanSource ./.;
          src = lib.cleanSourceWith {
            src = rootSrc;
            name = "takusu-source";
            filter =
              path: type:
              let
                base = baseNameOf path;
                parent = baseNameOf (dirOf path);
                pathStr = toString path;
                srcPrefix = toString rootSrc + "/";
                isInCrates = lib.hasPrefix (srcPrefix + "crates/") pathStr;
                # rust-embed embeds web/dist at compile time, so the bundled
                # frontend assets must be part of the crate source.
                isInWebDist = lib.hasPrefix (srcPrefix + "web/dist/") pathStr;
                excludedDirs = [
                  "mobile"
                  "scripts"
                  ".devin"
                  ".github"
                  ".serena"
                  "design"
                  ".wrangler"
                  "target"
                  "node_modules"
                ];
                isExtraFile =
                  (
                    isInCrates
                    && (lib.any (ext: lib.hasSuffix ".${ext}" base) [
                      "sql"
                      "json"
                      "ics"
                    ])
                  )
                  || (lib.hasSuffix ".md" base && parent == "skills")
                  || isInWebDist;
              in
              if type == "directory" then
                !(lib.elem base excludedDirs)
              else
                (craneLib.filterCargoSources path type) || isExtraFile;
          };

          inherit (craneLib.crateNameFromCargoToml { inherit src; }) version;
          cargoArtifacts = craneLib.buildDepsOnly {
            inherit src;
            strictDeps = true;
            pname = "takusu-deps";
            # takusu-audio pulls in lindera-ipadic, which downloads a dictionary
            # at build time. Provide it offline so the deps build succeeds.
            preBuild = ''
              export LINDERA_DICTIONARIES_PATH="$NIX_BUILD_TOP/lindera-dicts"
              mkdir -p "$LINDERA_DICTIONARIES_PATH/4.0.1"
              cp --no-preserve=mode "${linderaIpadicDict}/4.0.1/mecab-ipadic-2.7.0-20250920.tar.gz" "$LINDERA_DICTIONARIES_PATH/4.0.1/"
            '';
            nativeBuildInputs = with pkgs; [
              pkg-config
              cmake
              libclang
            ];
            buildInputs =
              with pkgs;
              [
                alsa-lib
                libpulseaudio
                openblas
              ]
              ++ [ sherpaOnnxLinuxX64Shared ];
            LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
            OPENBLAS_PATH = "${pkgs.openblas}/lib";
            BLAS_INCLUDE_DIRS = "${pkgs.openblas.dev}/include";
            SHERPA_ONNX_LIB_DIR = "${sherpaOnnxLinuxX64Shared}/lib";
          };

          takusu-cli = craneLib.buildPackage {
            inherit src cargoArtifacts;
            strictDeps = true;
            pname = "takusu-cli";
            cargoExtraArgs = "-p takusu-cli";
            # takusu-audio pulls in lindera-ipadic, which downloads a dictionary
            # at build time. Provide it offline so the build succeeds in the sandbox.
            preBuild = ''
              export LINDERA_DICTIONARIES_PATH="$NIX_BUILD_TOP/lindera-dicts"
              mkdir -p "$LINDERA_DICTIONARIES_PATH/4.0.1"
              cp --no-preserve=mode "${linderaIpadicDict}/4.0.1/mecab-ipadic-2.7.0-20250920.tar.gz" "$LINDERA_DICTIONARIES_PATH/4.0.1/"
            '';
            # Tests that spawn an in-process HTTP client need CA certificates,
            # which are not available in the Nix build sandbox. CI already runs
            # the full test suite separately, so skip checks here.
            doCheck = false;
            nativeBuildInputs = with pkgs; [
              pkg-config
              cmake
              libclang
            ];
            buildInputs =
              with pkgs;
              [
                alsa-lib
                libpulseaudio
                openblas
              ]
              ++ [ sherpaOnnxLinuxX64Shared ];
            LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
            OPENBLAS_PATH = "${pkgs.openblas}/lib";
            BLAS_INCLUDE_DIRS = "${pkgs.openblas.dev}/include";
            SHERPA_ONNX_LIB_DIR = "${sherpaOnnxLinuxX64Shared}/lib";
            meta.mainProgram = "takusu";
          };

          takusu-local = craneLib.buildPackage {
            inherit src cargoArtifacts;
            strictDeps = true;
            pname = "takusu-local";
            cargoExtraArgs = "-p takusu-local";
            nativeBuildInputs = with pkgs; [
              pkg-config
              cmake
              libclang
            ];
            buildInputs =
              with pkgs;
              [
                alsa-lib
                libpulseaudio
                openblas
              ]
              ++ [ sherpaOnnxLinuxX64Shared ];
            LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
            OPENBLAS_PATH = "${pkgs.openblas}/lib";
            BLAS_INCLUDE_DIRS = "${pkgs.openblas.dev}/include";
            SHERPA_ONNX_LIB_DIR = "${sherpaOnnxLinuxX64Shared}/lib";
          };

          # Shared @takusu/client package built in Nix (pure tsup build, no
          # file: deps). Output is package.json + dist/ so it can be dropped
          # into node_modules/@takusu/client for the web frontend build.
          tsClientSrc = lib.cleanSourceWith {
            src = ./ts/takusu-client;
            filter =
              path: type:
              let
                base = baseNameOf path;
              in
              base != "node_modules" && base != "dist";
          };

          takusu-client-npm = pkgs.buildNpmPackage {
            pname = "takusu-client";
            inherit version;
            src = tsClientSrc;
            npmDeps = pkgs.fetchNpmDeps {
              name = "takusu-client-npm-deps";
              src = tsClientSrc;
              hash = "sha256-DoRuYiYcaYIpS09LHLPA7pMcqtUNuolWO+oacQFkphE=";
            };
            buildPhase = ''
              npm run build
            '';
            installPhase = ''
              mkdir -p $out
              cp package.json $out/
              cp -r dist $out/
            '';
            dontFixup = true;
          };

          # Frontend bundle for takusu-web. Builds web/ with Vite inside the
          # Nix sandbox (npm deps prefetched via fetchNpmDeps). The resulting
          # dist/ is embedded into the takusu-web binary by rust-embed.
          webFrontendSrc = lib.cleanSourceWith {
            src = ./web;
            filter =
              path: type:
              let
                base = baseNameOf path;
              in
              base != "node_modules" && base != "dist";
          };

          takusu-web-frontend = pkgs.buildNpmPackage {
            pname = "takusu-web-frontend";
            inherit version;
            src = webFrontendSrc;
            npmDeps = pkgs.fetchNpmDeps {
              name = "takusu-web-npm-deps";
              src = webFrontendSrc;
              hash = "sha256-CE4pr6fugzBu2YOgQNhqLoR8mPyhUcVYfTAyUfecAro=";
            };
            # web depends on @takusu/client via `file:../ts/takusu-client`,
            # which does not exist inside the sandbox and which `npm ci` cannot
            # resolve. Drop the Nix-built client straight into node_modules
            # before Vite resolves the import. (Done in buildPhase rather than a
            # preBuild hook because buildNpmPackage does not forward preBuild.)
            buildPhase = ''
              mkdir -p node_modules/@takusu
              rm -rf node_modules/@takusu/client
              cp -r ${takusu-client-npm} node_modules/@takusu/client
              chmod -R +w node_modules/@takusu/client
              npm run build
            '';
            installPhase = ''
              cp -r dist $out
            '';
            dontFixup = true;
          };

          # Web UI server. Embeds the Vite-built frontend (takusu-web-frontend)
          # via rust-embed. The committed web/dist placeholder is swapped for
          # the real bundle in preBuild, so `nix build .#takusu-web` produces a
          # binary with the actual UI on a clean checkout.
          takusu-web = craneLib.buildPackage {
            inherit src cargoArtifacts;
            strictDeps = true;
            pname = "takusu-web";
            cargoExtraArgs = "-p takusu-web";
            preBuild = ''
              rm -rf web/dist
              cp -r ${takusu-web-frontend} web/dist
              chmod -R +w web/dist
            '';
            nativeBuildInputs = with pkgs; [
              pkg-config
              cmake
              libclang
            ];
            buildInputs =
              with pkgs;
              [
                alsa-lib
                libpulseaudio
                openblas
              ]
              ++ [ sherpaOnnxLinuxX64Shared ];
            LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
            OPENBLAS_PATH = "${pkgs.openblas}/lib";
            BLAS_INCLUDE_DIRS = "${pkgs.openblas.dev}/include";
            SHERPA_ONNX_LIB_DIR = "${sherpaOnnxLinuxX64Shared}/lib";
            meta.mainProgram = "takusu-web";
          };

          # Pre-fetched Lindera IPAdic dictionary source. lindera-ipadic's build
          # script downloads this tarball, which fails in the Nix sandbox because
          # no CA certificates are loaded. We provide it via LINDERA_DICTIONARIES_PATH.
          linderaIpadicDict = pkgs.runCommand "lindera-ipadic-dict" { } ''
            mkdir -p $out/4.0.1
            cp ${pkgs.fetchurl {
              url = "https://Lindera.dev/mecab-ipadic-2.7.0-20250920.tar.gz";
              hash = "sha256-p7qfZF/+cJTlauHEqB0QDfj7seKLvheSYi6XKOFi2z0=";
            }} $out/4.0.1/mecab-ipadic-2.7.0-20250920.tar.gz
          '';

          # Cross-compile takusu-android .so for a list of Android targets.
          # Uses crane only for cargo registry vendoring (no network needed in
          # the Nix sandbox). We can't use crane's buildDepsOnly because it
          # builds for the host target — the artifacts are useless for Android
          # cross-compilation. Instead, stdenv.mkDerivation + cargo-ndk builds
          # everything from the vendored registry in one derivation.
          mkTakusuAndroidLibs =
            { pname, androidTargets }:
            let
              vendoredDeps = craneLib.vendorCargoDeps { inherit src; };
            in
            pkgs.stdenv.mkDerivation {
              inherit src version pname;

              nativeBuildInputs = [
                rust-bin
                pkgs.cargo-ndk
                pkgs.pkg-config
                pkgs.cmake
                pkgs.libclang
              ];

              buildInputs = [
                androidComposition.ndk-bundle
              ];

              env = {
                ANDROID_NDK_HOME = androidNdkHome;
                LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
                OPENBLAS_PATH = "${pkgs.openblas}/lib";
                BLAS_INCLUDE_DIRS = "${pkgs.openblas.dev}/include";
                # cargo-ndk exports the Android clang as CC. Keep host C
                # build scripts (bzip2/ring) on the host compiler.
                "CC_x86_64-unknown-linux-gnu" = "${pkgs.stdenv.cc}/bin/cc";
                C_x86_64_unknown_linux_gnu = "${pkgs.stdenv.cc}/bin/cc";
                # tikv-jemalloc-sys emits -lgcc for Android, but newer NDKs
                # replaced libgcc with libunwind. Add a shim that redirects
                # -lgcc to -lunwind so jemalloc still links. Use target-specific
                # rustflags so host build scripts are not affected.
                CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS = "-L ${androidLibgccShim}/lib";
                CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_RUSTFLAGS = "-L ${androidLibgccShim}/lib";
                CARGO_TARGET_X86_64_LINUX_ANDROID_RUSTFLAGS = "-L ${androidLibgccShim}/lib";
                CARGO_TARGET_I686_LINUX_ANDROID_RUSTFLAGS = "-L ${androidLibgccShim}/lib";
              };

              # Don't run tests — cross-compiled binaries can't execute on host.
              doCheck = false;

              # lindera-ipadic tries to download its dictionary at build time.
              # Provide the pre-fetched tarball so the build succeeds offline.
              preBuild = ''
                export LINDERA_DICTIONARIES_PATH="$NIX_BUILD_TOP/lindera-dicts"
                mkdir -p "$LINDERA_DICTIONARIES_PATH/4.0.1"
                cp --no-preserve=mode "${linderaIpadicDict}/4.0.1/mecab-ipadic-2.7.0-20250920.tar.gz" "$LINDERA_DICTIONARIES_PATH/4.0.1/"
              '';

              # Override the default configurePhase (which would try to run
              # cmake since it's in nativeBuildInputs). We only need to set up
              # CARGO_HOME from crane's vendored deps.
              configurePhase = ''
                runHook preConfigure
                export CARGO_HOME="$NIX_BUILD_TOP/.cargo"
                mkdir -p "$CARGO_HOME"
                cp -r "${vendoredDeps}"/* "$CARGO_HOME/"
                runHook postConfigure
              '';

              buildPhase = ''
                runHook preBuild
                for target in ${lib.concatStringsSep " " androidTargets}; do
                  case $target in
                    aarch64-linux-android) sherpa_abi="arm64-v8a" ;;
                    armv7-linux-androideabi) sherpa_abi="armeabi-v7a" ;;
                    x86_64-linux-android) sherpa_abi="x86_64" ;;
                    i686-linux-android) sherpa_abi="x86" ;;
                  esac
                  echo "Building $target..."
                  SHERPA_ONNX_LIB_DIR="${sherpaOnnxAndroid}/jniLibs/$sherpa_abi" cargo ndk -t "$target" build -p takusu-android --release --no-default-features
                done
                runHook postBuild
              '';

              installPhase = ''
                runHook preInstall
                for target in ${lib.concatStringsSep " " androidTargets}; do
                  case $target in
                    aarch64-linux-android) abi_dir="arm64-v8a" ;;
                    armv7-linux-androideabi) abi_dir="armeabi-v7a" ;;
                    x86_64-linux-android) abi_dir="x86_64" ;;
                    i686-linux-android) abi_dir="x86" ;;
                  esac
                  mkdir -p "$out/jniLibs/$abi_dir"
                  cp "target/$target/release/libtakusu_android.so" "$out/jniLibs/$abi_dir/"
                  cp "${sherpaOnnxAndroid}/jniLibs/$abi_dir"/lib*.so "$out/jniLibs/$abi_dir/"
                done
                runHook postInstall
              '';
            };

          takusu-android-libs = mkTakusuAndroidLibs {
            pname = "takusu-android-libs";
            # Only build arm64-v8a (aarch64-linux-android) for release. Modern
            # Android devices are arm64; dropping armeabi-v7a / x86 / x86_64
            # cuts the Rust cross-compile and Gradle CMake build time.
            androidTargets = [ "aarch64-linux-android" ];
          };

          takusu-android-libs-emulator = mkTakusuAndroidLibs {
            pname = "takusu-android-libs-emulator";
            androidTargets = [ "x86_64-linux-android" ];
          };

          # Host-native uniffi-bindgen binary for generating Kotlin bindings.
          # Built with the `bindgen` feature which pulls in uniffi/cli.
          uniffi-bindgen = craneLib.buildPackage {
            inherit src cargoArtifacts version;
            pname = "uniffi-bindgen";
            cargoExtraArgs = "-p takusu-android --features bindgen --bin uniffi-bindgen";
            # takusu-android pulls in takusu-audio -> lindera-ipadic, which
            # downloads a dictionary at build time. Provide it offline.
            preBuild = ''
              export LINDERA_DICTIONARIES_PATH="$NIX_BUILD_TOP/lindera-dicts"
              mkdir -p "$LINDERA_DICTIONARIES_PATH/4.0.1"
              cp --no-preserve=mode "${linderaIpadicDict}/4.0.1/mecab-ipadic-2.7.0-20250920.tar.gz" "$LINDERA_DICTIONARIES_PATH/4.0.1/"
            '';
            nativeBuildInputs = with pkgs; [
              pkg-config
              cmake
              libclang
            ];
            buildInputs =
              with pkgs;
              [
                alsa-lib
                libpulseaudio
                openblas
              ]
              ++ [ sherpaOnnxLinuxX64Shared ];
            LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
            OPENBLAS_PATH = "${pkgs.openblas}/lib";
            BLAS_INCLUDE_DIRS = "${pkgs.openblas.dev}/include";
            SHERPA_ONNX_LIB_DIR = "${sherpaOnnxLinuxX64Shared}/lib";
            doCheck = false;
          };

          mcp-servers = import inputs.mcp-servers-nix { inherit pkgs; };

          mcp-config = mcp-servers.lib.mkConfig pkgs {
            # Devin CLI reads `.devin/config.json` with the same `mcpServers`
            # shape as Claude Desktop (command + args separated), so we use the
            # `claude` flavor. The `opencode` flavor emits `command` as a single
            # array and uses `environment`/`type`/`enabled`, which Devin doesn't
            # understand.
            flavor = "claude";
            fileName = "config.json";
            programs = {
              serena = {
                enable = true;
                context = "agent";
                extraPackages = [
                  pkgs.rust-analyzer
                  pkgs.nixd
                ];
              };
              context7.enable = true;
              # GitHub MCP server (stdio). The passwordCommand wraps the
              # binary so GITHUB_PERSONAL_ACCESS_TOKEN is fetched at runtime
              # via `gh auth token` — no secret lands in the Nix store.
              github = {
                enable = true;
                passwordCommand = {
                  GITHUB_PERSONAL_ACCESS_TOKEN = [
                    "${lib.getExe pkgs.gh}"
                    "auth"
                    "token"
                  ];
                };
              };
            };
          };

          # Common shell environment for Rust-based CI jobs. Sets the env vars
          # needed by crates that link against native libs (alsa, pulse,
          # openblas, libclang for bindgen, etc.). Factored out so the per-job
          # devShells stay in sync with the default devShell.
          commonRustShellHook = ''
            export LIBCLANG_PATH=${pkgs.libclang.lib}/lib
            export OPENBLAS_PATH=${pkgs.openblas}/lib
            export BLAS_INCLUDE_DIRS=${pkgs.openblas.dev}/include
            export LD_LIBRARY_PATH="${pkgs.stdenv.cc.cc.lib}/lib:${pkgs.openssl.out}/lib:${pkgs.openblas}/lib:${pkgs.zlib}/lib''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
            export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=${pkgs.stdenv.cc}/bin/cc
          '';
        in
        {
          _module.args.pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = [
              inputs.rust-overlay.overlays.default
            ];
            config = {
              allowUnfree = true;
              permittedUnfreePackages = [ "android-sdk-ndk" ];
            };
          };

          treefmt = {
            projectRootFile = "flake.nix";

            programs = {
              nixfmt.enable = true;
              rustfmt = {
                enable = true;
                package = rust-bin;
              };
              actionlint.enable = true;
            };
          };

          # Shared build logic for the Android APK, factored out so the stable
          # and dev variants share the same steps. app.config.js reads
          # TAKUSU_BUILD_VARIANT to switch the application ID / launcher label
          # / deep-link scheme so a dev build can coexist with the stable app
          # on the same device.
          #
          # Variant is "" for stable (dev.satler.takusu) and "dev" for the
          # development build (dev.satler.takusu.dev).
          packages =
            let
              androidApkRuntimeInputs = [
                pkgs.nodejs
                pkgs.openjdk_headless
                androidComposition.ndk-bundle
                androidComposition.androidsdk
              ];
              makeAndroidBuildText = finalStep: androidLibs: abiDir: ''
                export ANDROID_NDK_HOME="${androidNdkHome}"
                export ANDROID_HOME="${androidComposition.androidsdk}/libexec/android-sdk"
                # React Native's Gradle plugin errors out when ANDROID_HOME and
                # ANDROID_SDK_ROOT point to different SDKs. GitHub Actions
                # runners pre-set ANDROID_SDK_ROOT to the system SDK, so align
                # it with our Nix-managed ANDROID_HOME to avoid the conflict.
                export ANDROID_SDK_ROOT="$ANDROID_HOME"
                export JAVA_HOME="${pkgs.openjdk_headless}/lib/openjdk"
                export LD_LIBRARY_PATH="${pkgs.stdenv.cc.cc.lib}/lib:${pkgs.openssl.out}/lib:${pkgs.openblas}/lib:${pkgs.zlib}/lib''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

                if [ ! -f "scripts/post-prebuild-android.sh" ]; then
                  echo "Error: run from the takusu repo root" >&2
                  exit 1
                fi

                # Embed git commit/tag into the APK via app.config.js so the
                # settings page can show the exact source the build came from.
                # Allow caller overrides (CI may set these explicitly).
                : "''${TAKUSU_GIT_COMMIT:=$(git -C "$(pwd)" rev-parse --short HEAD 2>/dev/null || echo unknown)}"
                : "''${TAKUSU_GIT_TAG:=$(git -C "$(pwd)" describe --tags --always 2>/dev/null || echo unknown)}"
                export TAKUSU_GIT_COMMIT TAKUSU_GIT_TAG

                REPO_ROOT="$(pwd)"
                MODULE_DIR="$REPO_ROOT/mobile/modules/takusu-server"
                JNILIBS_DIR="$MODULE_DIR/android/src/main/jniLibs"
                BINDINGS_DIR="$MODULE_DIR/android/src/main/java/uniffi/takusu_android"

                # 1. Copy pre-built .so from Nix store into jniLibs/
                echo "── Step 1: Copy native libraries from Nix store ──"
                if [ -d "$JNILIBS_DIR" ]; then
                  chmod -R +w "$JNILIBS_DIR"
                fi
                rm -rf "$JNILIBS_DIR"
                mkdir -p "$JNILIBS_DIR"
                cp -r --no-preserve=mode "${androidLibs}/jniLibs/." "$JNILIBS_DIR/"
                echo "  copied .so files to $JNILIBS_DIR"

                # 2. Generate UniFFI Kotlin bindings from the .so
                echo ""
                echo "── Step 2: Generate UniFFI Kotlin bindings ──"
                BINDINGS_TMP="$BINDINGS_DIR/uniffi"
                mkdir -p "$BINDINGS_TMP"
                "${uniffi-bindgen}/bin/uniffi-bindgen" generate \
                  --library "${androidLibs}/jniLibs/${abiDir}/libtakusu_android.so" \
                  --language kotlin \
                  --out-dir "$BINDINGS_TMP"
                GENERATED_FILE=$(find "$BINDINGS_TMP" -name "*.kt" -type f | head -1)
                if [ -n "$GENERATED_FILE" ]; then
                  mv "$GENERATED_FILE" "$BINDINGS_DIR/"
                  rm -rf "$BINDINGS_TMP"
                  echo "  generated bindings in $BINDINGS_DIR"
                else
                  echo "  Error: no Kotlin bindings generated" >&2
                  exit 1
                fi

                # 3. Expo prebuild (generates android/ directory)
                #    app.config.js reads TAKUSU_BUILD_VARIANT to switch the
                #    application ID / scheme / launcher label for dev builds.
                echo ""
                echo "── Step 3: Expo prebuild (variant=''${TAKUSU_BUILD_VARIANT:-stable}) ──"
                cd mobile
                npx expo prebuild --platform android --no-install

                # 4. Apply post-prebuild fixes (Gradle pin, NDK override, etc.)
                echo ""
                echo "── Step 4: Post-prebuild fixes ──"
                "$REPO_ROOT/scripts/post-prebuild-android.sh" android

                # 5. ${finalStep.label}
                echo ""
                echo "── Step 5: ${finalStep.label} ──"
                cd android
                ${finalStep.command}

                echo ""
                echo "${finalStep.success}"
              '';
              apkFinalStep = {
                label = "Gradle assembleRelease";
                command = "./gradlew :app:assembleRelease --stacktrace";
                success = "✅ APK built: $(pwd)/app/build/outputs/apk/release/app-release.apk";
              };
              unitTestFinalStep = {
                label = "Gradle unit tests";
                command = "./gradlew :takusu-widget:testDebugUnitTest :takusu-server:testDebugUnitTest --stacktrace";
                success = "✅ Unit tests passed";
              };
              makeAndroidApkBuildText = makeAndroidBuildText apkFinalStep;
              androidApkBuildText = makeAndroidApkBuildText takusu-android-libs "arm64-v8a";
              androidApkEmulatorBuildText = makeAndroidApkBuildText takusu-android-libs-emulator "x86_64";
              androidUnitTestText = makeAndroidBuildText unitTestFinalStep takusu-android-libs "arm64-v8a";
            in
            {
              inherit
                takusu-cli
                takusu-local
                takusu-web
                takusu-android-libs
                takusu-android-libs-emulator
                uniffi-bindgen
                ;
              default = takusu-cli;

              # Full APK build script. Run from the repo root:
              #   nix run .#build-android-apk
              # Produces: mobile/android/app/build/outputs/apk/release/app-release.apk
              #
              # Uses pre-built .so and uniffi-bindgen from the Nix store.
              build-android-apk = pkgs.writeShellApplication {
                name = "build-android-apk";
                runtimeInputs = androidApkRuntimeInputs;
                text = androidApkBuildText;
              };

              # Development APK build. Same as build-android-apk but sets
              # TAKUSU_BUILD_VARIANT=dev so app.config.js emits a distinct
              # application ID (dev.satler.takusu.dev), launcher label
              # ("takusu dev"), and scheme ("takusu-dev"). This lets the dev
              # build be installed alongside the stable app on a real device.
              build-android-apk-dev = pkgs.writeShellApplication {
                name = "build-android-apk-dev";
                runtimeInputs = androidApkRuntimeInputs;
                text = ''
                  export TAKUSU_BUILD_VARIANT=dev
                  ${androidApkBuildText}
                '';
              };

              # Emulator APK build. Builds x86_64 native libs so the app runs
              # on the x86_64 emulator (the host architecture for KVM).
              # Run with `nix run .#build-android-apk-emulator`.
              build-android-apk-emulator = pkgs.writeShellApplication {
                name = "build-android-apk-emulator";
                runtimeInputs = androidApkRuntimeInputs;
                text = ''
                  export TAKUSU_BUILD_VARIANT=dev
                  export TAKUSU_ANDROID_ABIS=x86_64
                  ${androidApkEmulatorBuildText}
                '';
              };

              # Android library unit tests (Robolectric/JUnit4). Run with:
              #   nix run .#test-android-unit
              #
              # Prebuilds Expo, generates UniFFI bindings, and runs
              # :takusu-widget:testDebugUnitTest and :takusu-server:testDebugUnitTest.
              test-android-unit = pkgs.writeShellApplication {
                name = "test-android-unit";
                runtimeInputs = androidApkRuntimeInputs;
                text = androidUnitTestText;
              };

              # Android emulator for local development. Creates an AVD on first
              # run and launches an x86_64 emulator (matching the host for KVM).
              # Run with `nix run .#android-emulator`.
              android-emulator = pkgs.writeShellApplication {
                name = "android-emulator";
                runtimeInputs = [
                  emulatorSdk
                  pkgs.openjdk_headless
                  pkgs.coreutils
                  pkgs.gnugrep
                ];
                text = ''
                  #!/usr/bin/env bash
                  set -euo pipefail

                  export ANDROID_HOME="${emulatorSdk}/libexec/android-sdk"
                  export ANDROID_SDK_ROOT="$ANDROID_HOME"
                  export JAVA_HOME="${pkgs.openjdk_headless}/lib/openjdk"

                  DEVICE_NAME="''${TAKUSU_EMULATOR_DEVICE:-takusu}"
                  API_LEVEL="''${TAKUSU_EMULATOR_API:-35}"
                  IMAGE_TYPE="''${TAKUSU_EMULATOR_IMAGE:-google_apis}"
                  ABI="''${TAKUSU_EMULATOR_ABI:-x86_64}"
                  SYSTEM_IMAGE="system-images;android-$API_LEVEL;$IMAGE_TYPE;$ABI"
                  DEFAULT_FLAGS=( -no-boot-anim -gpu swiftshader_indirect )
                  if [ -n "''${TAKUSU_EMULATOR_DEFAULT_FLAGS:-}" ]; then
                    read -r -a DEFAULT_FLAGS <<< "''${TAKUSU_EMULATOR_DEFAULT_FLAGS:-}"
                  fi
                  EXTRA_FLAGS=()
                  if [ -n "''${TAKUSU_EMULATOR_FLAGS:-}" ]; then
                    read -r -a EXTRA_FLAGS <<< "''${TAKUSU_EMULATOR_FLAGS:-}"
                  fi
                  ANDROID_USER_HOME="''${TAKUSU_EMULATOR_USER_HOME:-$HOME/.takusu/android}"
                  ANDROID_AVD_HOME="$ANDROID_USER_HOME/avd"
                  export ANDROID_USER_HOME ANDROID_AVD_HOME

                  mkdir -p "$ANDROID_AVD_HOME"

                  AVD_DIR="$ANDROID_AVD_HOME/$DEVICE_NAME.avd"
                  if [ -d "$AVD_DIR" ]; then
                    existing_abi=$(grep '^abi.type=' "$AVD_DIR/config.ini" 2>/dev/null | cut -d= -f2 || true)
                    if [ "$existing_abi" != "$ABI" ]; then
                      echo "Existing AVD '$DEVICE_NAME' uses ABI '$existing_abi'; removing it to recreate with '$ABI'..."
                      rm -rf "$AVD_DIR"
                      rm -f "$ANDROID_AVD_HOME/$DEVICE_NAME.ini"
                    fi
                  fi

                  if [ ! -d "$AVD_DIR" ]; then
                    echo "Creating AVD '$DEVICE_NAME' with $SYSTEM_IMAGE..."
                    printf '\n' | avdmanager create avd \
                      --force \
                      -n "$DEVICE_NAME" \
                      -k "$SYSTEM_IMAGE" \
                      -p "$AVD_DIR"
                  fi

                  echo "Looking for a free emulator port in range 5554-5584..."
                  port=""
                  for i in $(seq 5554 2 5584); do
                    if ! adb devices | grep -q "emulator-$i"; then
                      port="$i"
                      break
                    fi
                  done

                  if [ -z "$port" ]; then
                    echo "No free emulator port found" >&2
                    exit 1
                  fi

                  echo "Launching emulator '$DEVICE_NAME' on port $port"
                  exec "$ANDROID_HOME/emulator/emulator" \
                    -avd "$DEVICE_NAME" \
                    -port "$port" \
                    "''${DEFAULT_FLAGS[@]}" \
                    "''${EXTRA_FLAGS[@]}" \
                    "$@"
                '';
              };

              # Per-job CI environments. Each job installs only what it needs so
              # the binary cache stays small and the runner doesn't fill its disk
              # with the Android SDK / Node / JVM on jobs that don't use them.
              # The devShells below mirror these so `nix develop .#<job>` only
              # pulls the same subset.
              ci-rust = pkgs.buildEnv {
                name = "ci-rust";
                paths = with pkgs; [
                  cargo-codspeed
                  cargo-expand
                  cargo-nextest
                  rust-bin
                  pkg-config
                  cmake
                  stdenv.cc
                  mold
                  alsa-lib
                  libpulseaudio
                  libclang
                  openblas
                  zlib
                  sherpaOnnxLinuxX64Shared
                ];
              };

              ci-android = pkgs.buildEnv {
                name = "ci-android";
                paths =
                  with pkgs;
                  [
                    cargo-ndk
                    rust-bin
                    pkg-config
                    cmake
                    libclang
                    stdenv.cc
                    openblas
                    zlib
                    sherpaOnnxAndroid
                  ]
                  ++ [
                    androidComposition.ndk-bundle
                    androidLibgccShim
                  ];
              };

              # Combined closure of the two Nix-built Android derivations
              # (cross-compiled .so + uniffi-bindgen) so they are cached and
              # restored together from the Nix store cache in CI.
              ci-android-libs = pkgs.buildEnv {
                name = "ci-android-libs";
                paths = [
                  takusu-android-libs
                  uniffi-bindgen
                ];
              };

              ci-worker = pkgs.buildEnv {
                name = "ci-worker";
                paths = with pkgs; [
                  rust-bin
                  wrangler
                  worker-build
                  pkg-config
                  cmake
                  libclang
                  stdenv.cc
                ];
              };

              # Kotlin lint/format (ktlint) for the mobile Android modules.
              # Used by the `kotlin-check` CI job and the `kotlin` devShell.
              ci-kotlin = pkgs.buildEnv {
                name = "ci-kotlin";
                paths = with pkgs; [
                  ktlint
                  openjdk_headless
                ];
              };
            };

          devShells = {
            # Minimal shells for CI jobs — each only pulls what the job needs so
            # `nix develop .#<job>` doesn't download the Android SDK / Node / JVM
            # on jobs that don't use them. The `ci-*` packages above mirror these
            # so the binary cache warms exactly the same store paths.
            rust = pkgs.mkShell {
              nativeBuildInputs =
                with pkgs;
                [
                  cargo-codspeed
                  cargo-expand
                  cargo-flamegraph
                  cargo-nextest
                  rust-bin
                  pkg-config
                  cmake
                  stdenv.cc
                  mold
                ]
                ++ lib.optional pkgs.stdenv.isLinux pkgs.perf;
              buildInputs = with pkgs; [
                alsa-lib
                libpulseaudio
                libclang
                openblas
                stdenv.cc.cc.lib
                zlib
                sherpaOnnxLinuxX64Shared
              ];
              shellHook = commonRustShellHook + ''
                # Copy Sherpa-ONNX shared libs into a writable directory so the
                # build.rs copy step can overwrite them on rebuilds (the Nix store
                # files are read-only and would cause "Permission denied").
                _setup_sherpa_host() {
                  local _sherpa_lib_dir="$PWD/target/sherpa-onnx-linux-x64-shared"
                  mkdir -p "$_sherpa_lib_dir"
                  cp -f "${sherpaOnnxLinuxX64Shared}/lib"/*.so "$_sherpa_lib_dir/"
                  chmod +w "$_sherpa_lib_dir"/*.so
                  export SHERPA_ONNX_LIB_DIR="$_sherpa_lib_dir"
                  export LD_LIBRARY_PATH="$_sherpa_lib_dir''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
                }
                _setup_sherpa_host
                unset -f _setup_sherpa_host
                # Remove stale read-only copies from previous builds so the
                # build.rs copy step can overwrite them.
                rm -f target/debug/libonnxruntime.so target/debug/libsherpa-onnx-*.so \
                  target/debug/examples/libonnxruntime.so target/debug/examples/libsherpa-onnx-*.so 2>/dev/null || true
              '';
            };

            android = pkgs.mkShell {
              nativeBuildInputs = with pkgs; [
                cargo-ndk
                rust-bin
                pkg-config
                cmake
                stdenv.cc
              ];
              buildInputs = with pkgs; [
                libclang
                openblas
                stdenv.cc.cc.lib
                zlib
                androidComposition.ndk-bundle
                sherpaOnnxAndroid
              ];
              shellHook = commonRustShellHook + ''
                export ANDROID_NDK_HOME=${androidNdkHome}
                export SHERPA_ONNX_LIB_DIR="${sherpaOnnxAndroid}/jniLibs/arm64-v8a"
                # cargo-ndk sets CC/CXX to the Android NDK clang for the target,
                # but build-dependencies (bzip2/ring) compile for the host. Keep
                # host builds on the Nix-provided host compiler.
                export HOST_CC="${pkgs.stdenv.cc}/bin/cc"
                export HOST_CXX="${pkgs.stdenv.cc}/bin/c++"
                export HOST_CFLAGS=""
                export HOST_CXXFLAGS=""
                # tikv-jemalloc-sys emits -lgcc for Android, but newer NDKs
                # replaced libgcc with libunwind. Add a shim that redirects
                # -lgcc to -lunwind so jemalloc still links. Use target-specific
                # rustflags so host build scripts are not affected.
                export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="-L ${androidLibgccShim}/lib"
                export CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_RUSTFLAGS="-L ${androidLibgccShim}/lib"
                export CARGO_TARGET_X86_64_LINUX_ANDROID_RUSTFLAGS="-L ${androidLibgccShim}/lib"
                export CARGO_TARGET_I686_LINUX_ANDROID_RUSTFLAGS="-L ${androidLibgccShim}/lib"
              '';
            };

            worker = pkgs.mkShell {
              nativeBuildInputs = with pkgs; [
                rust-bin
                wrangler
                worker-build
                pkg-config
                cmake
                stdenv.cc
              ];
              buildInputs = with pkgs; [ libclang ];
              shellHook = commonRustShellHook;
            };

            # Kotlin lint/format shell (ktlint). Used by `npm run kt:lint`
            # and `npm run kt:fmt` in mobile/.
            kotlin = pkgs.mkShell {
              nativeBuildInputs = with pkgs; [
                ktlint
                openjdk_headless
              ];
            };

            # Full shell for local development — keeps everything (Android SDK,
            # Node, JVM, MCP config symlink, etc.).
            default = pkgs.mkShell {
              nativeBuildInputs =
                with pkgs;
                [
                  cargo-codspeed
                  cargo-expand
                  cargo-flamegraph
                  cargo-nextest
                  cargo-ndk
                  rust-bin
                  pkg-config
                  cmake
                  stdenv.cc
                  mold
                  nodejs
                  wrangler
                  openjdk_headless
                  ktlint
                  (python3.withPackages (pythonPackages: [ pythonPackages.ortools ]))
                ]
                ++ lib.optional pkgs.stdenv.isLinux pkgs.perf;

              buildInputs = with pkgs; [
                alsa-lib
                libpulseaudio
                libclang
                openblas
                stdenv.cc.cc.lib
                zlib
                worker-build
                androidComposition.ndk-bundle
                sherpaOnnxLinuxX64Shared
                sherpaOnnxAndroid
              ];

              shellHook = commonRustShellHook + ''
                export ANDROID_NDK_HOME=${androidNdkHome}
                export ANDROID_HOME=${androidComposition.androidsdk}/libexec/android-sdk
                export JAVA_HOME=${pkgs.openjdk_headless}/lib/openjdk
                # Copy Sherpa-ONNX shared libs into a writable directory so the
                # build.rs copy step can overwrite them on rebuilds.
                _setup_sherpa_host() {
                  local _sherpa_lib_dir="$PWD/target/sherpa-onnx-linux-x64-shared"
                  mkdir -p "$_sherpa_lib_dir"
                  cp -f "${sherpaOnnxLinuxX64Shared}/lib"/*.so "$_sherpa_lib_dir/"
                  chmod +w "$_sherpa_lib_dir"/*.so
                  export SHERPA_ONNX_LIB_DIR="$_sherpa_lib_dir"
                  export LD_LIBRARY_PATH="$_sherpa_lib_dir''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
                }
                _setup_sherpa_host
                unset -f _setup_sherpa_host
                # Remove stale read-only copies from previous builds.
                rm -f target/debug/libonnxruntime.so target/debug/libsherpa-onnx-*.so \
                  target/debug/examples/libonnxruntime.so target/debug/examples/libsherpa-onnx-*.so 2>/dev/null || true
                if [ -L .devin/config.json ]; then
                  unlink .devin/config.json
                fi
                mkdir -p .devin
                ln -sf ${mcp-config} .devin/config.json

                # tikv-jemalloc-sys emits -lgcc for Android, but newer NDKs
                # replaced libgcc with libunwind. Add a shim that redirects
                # -lgcc to -lunwind so jemalloc still links. Use target-specific
                # rustflags so host build scripts are not affected.
                export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="-L ${androidLibgccShim}/lib"
                export CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_RUSTFLAGS="-L ${androidLibgccShim}/lib"
                export CARGO_TARGET_X86_64_LINUX_ANDROID_RUSTFLAGS="-L ${androidLibgccShim}/lib"
                export CARGO_TARGET_I686_LINUX_ANDROID_RUSTFLAGS="-L ${androidLibgccShim}/lib"
              '';
            };
          };
        };
    };
}
