{
  description = "hunkle - staging helper";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    git-hooks-nix = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs:
    let
      pname = "hunkle";
      version = "0.3.0";

      mkHunkle = pkgs: pkgs.rustPlatform.buildRustPackage {
        inherit pname version;
        src = inputs.self;
        cargoLock.lockFile = inputs.self + "/Cargo.lock";
        nativeCheckInputs = [ pkgs.git pkgs.openssh ];
        meta.mainProgram = "hunkle";
      };

      mkHunkleEl = epkgs: bin: epkgs.trivialBuild {
        inherit pname version;
        src = inputs.self + "/emacs";
        packageRequires = [ epkgs.magit ];
        postPatch = ''
          substituteInPlace hunkle.el \
            --replace-fail '(defcustom hunkle-executable "hunkle"' \
              '(defcustom hunkle-executable "${bin}/bin/hunkle"'
        '';
      };
    in
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      imports = [
        inputs.treefmt-nix.flakeModule
        inputs.git-hooks-nix.flakeModule
      ];

      systems = [ "x86_64-linux" "aarch64-linux" ];

      flake.overlays.default = final: prev: {
        hunkle = mkHunkle final;
        emacsPackagesFor = emacs:
          (prev.emacsPackagesFor emacs).overrideScope
            (efinal: _eprev: { hunkle = mkHunkleEl efinal final.hunkle; });
      };

      perSystem = { config, self', pkgs, ... }:
        let
          emacsWithHunkle = (pkgs.emacsPackagesFor pkgs.emacs).emacsWithPackages
            (epkgs: [ epkgs.magit (mkHunkleEl epkgs self'.packages.hunkle) ]);

          demoRepoSetup = ''
            repo="$(mktemp -d -t hunkle-demo.XXXXXX)"
            trap 'rm -rf "$repo"' EXIT
            cd "$repo"
            git init -q -b main
            git config user.name demo
            git config user.email demo@example.com
            git config commit.gpgsign false

            seq 30 | sed 's/^/alpha line /' > a.txt
            seq 30 | sed 's/^/beta line /' > b.txt
            git add -A
            git commit -qm base

            sed -i '3s/$/ (edited)/;25s/$/ (edited)/' a.txt
            sed -i '12s/$/ (edited)/' b.txt
            printf 'a brand new file\nwith two lines\n' > c.txt
            git add -A

            echo "hunkle demo repo: $repo (removed on exit)"
          '';

          demo = pkgs.writeShellApplication {
            name = "hunkle-demo";
            runtimeInputs = [ pkgs.git pkgs.coreutils pkgs.gnused self'.packages.hunkle ];
            excludeShellChecks = [ "SC2016" ];
            text = demoRepoSetup + ''
              hunkle "$@"
            '';
          };

          demo-emacs = pkgs.writeShellApplication {
            name = "hunkle-demo-emacs";
            runtimeInputs = [ pkgs.git pkgs.coreutils pkgs.gnused emacsWithHunkle ];
            excludeShellChecks = [ "SC2016" ];
            text = demoRepoSetup + ''
              emacs -q \
                --eval "(progn (require 'hunkle) (hunkle-magit-setup) (setq default-directory \"$repo/\") (hunkle))" \
                "$@"
            '';
          };

          elispFmt = pkgs.writeText "hunkle-elisp-fmt.el" ''
            (require 'magit)
            (put 'hunkle-test--with-buffer 'lisp-indent-function 1)
            (while command-line-args-left
              (let ((file (pop command-line-args-left)))
                (with-current-buffer (find-file-noselect file)
                  (emacs-lisp-mode)
                  (setq indent-tabs-mode nil)
                  (indent-region (point-min) (point-max))
                  (delete-trailing-whitespace)
                  (save-buffer))))
          '';
        in
        {
          treefmt = {
            programs = {
              nixpkgs-fmt.enable = true;
              rustfmt.enable = true;
              deadnix.enable = true;
              statix.enable = true;
            };
            settings.formatter.elisp = {
              command = "${emacsWithHunkle}/bin/emacs";
              options = [ "--batch" "-l" "${elispFmt}" ];
              includes = [ "*.el" ];
            };
          };

          pre-commit.settings = {
            settings = {
              rust = {
                cargoManifestPath = "./Cargo.toml";
                check.cargoDeps = pkgs.rustPlatform.importCargoLock { lockFile = ./Cargo.lock; };
              };
            };
            hooks = {
              treefmt.enable = true;
              clippy.enable = true;
            };
          };

          packages = rec {
            hunkle = mkHunkle pkgs;
            hunkle-emacs = mkHunkleEl pkgs.emacsPackages hunkle;
            inherit demo demo-emacs;
            default = hunkle;
          };

          apps = {
            demo = {
              type = "app";
              program = pkgs.lib.getExe demo;
            };
            demo-emacs = {
              type = "app";
              program = pkgs.lib.getExe demo-emacs;
            };
          };

          checks = {
            rust-tests = self'.packages.hunkle;
            emacs-tests =
              pkgs.runCommandLocal "hunkle-emacs-tests"
                {
                  nativeBuildInputs = [ emacsWithHunkle pkgs.git ];
                  HUNKLE_BIN = pkgs.lib.getExe self'.packages.hunkle;
                }
                ''
                  export HOME="$TMPDIR"
                  export GIT_CONFIG_NOSYSTEM=1
                  emacs --batch \
                    -l ${inputs.self}/tests/hunkle-test.el \
                    -f ert-run-tests-batch-and-exit
                  touch "$out"
                '';
          };

          devShells.default = pkgs.mkShell {
            inputsFrom = [ self'.packages.default ];
            nativeBuildInputs = [ emacsWithHunkle ] ++ (with pkgs; [
              rust-analyzer
              clippy
              rustfmt
              git
            ]);
            shellHook = config.pre-commit.installationScript;
          };
        };
    };
}
