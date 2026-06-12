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
      version = "0.1.0";

      mkHunkle = pkgs: pkgs.rustPlatform.buildRustPackage {
        inherit pname version;
        src = inputs.self;
        cargoLock.lockFile = inputs.self + "/Cargo.lock";
        nativeCheckInputs = [ pkgs.git ];
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
          emacsWithMagit = (pkgs.emacsPackagesFor pkgs.emacs).emacsWithPackages
            (epkgs: [ epkgs.magit ]);

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
              command = "${emacsWithMagit}/bin/emacs";
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
            default = hunkle;
          };

          checks = {
            rust-tests = self'.packages.hunkle;
            emacs-tests =
              pkgs.runCommandLocal "hunkle-emacs-tests"
                {
                  nativeBuildInputs = [ emacsWithMagit pkgs.git ];
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
            nativeBuildInputs = [ emacsWithMagit ] ++ (with pkgs; [
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
