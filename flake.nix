{
  description = "Samply";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    flake-utils.url = "github:numtide/flake-utils";

    advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs = {
        nixpkgs.follows = "nixpkgs";
        flake-utils.follows = "flake-utils";
      };
    };

  };

  outputs = { self, nixpkgs, crane, flake-utils, advisory-db, ... }@inputs:
    flake-utils.lib.eachSystem [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ] (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [
            (import inputs.rust-overlay)
          ];
        };

        inherit (pkgs) lib;


        toolchain-settings = {
          extensions = [ "rust-src" ];
          targets = [ "aarch64-apple-darwin" "x86_64-apple-darwin" "aarch64-unknown-linux-gnu" "x86_64-unknown-linux-gnu" ];
        };
        # stable
        rust-toolchain = pkgs.rust-bin.stable.latest.default.override toolchain-settings;
        # nightly
        # rust-toolchain = pkgs.rust-bin.selectLatestNightlyWith (toolchain: toolchain.default.override toolchain-settings);

        craneLib = (crane.mkLib pkgs).overrideToolchain rust-toolchain;

        src = craneLib.cleanCargoSource ./.;

        buildInputs = with pkgs; [
          # Add additional build inputs here
          openssl
          pkg-config
        ];



        # Build *just* the cargo dependencies, so we can reuse
        # all of that work (e.g. via cachix) when running in CI
        cargoArtifacts = craneLib.buildDepsOnly {
          inherit src buildInputs;
        };

        # Build the actual crate itself, reusing the dependency
        # artifacts from above.
        samply = craneLib.buildPackage {
          inherit cargoArtifacts src buildInputs;
          pname = "samply";
          cargoExtraArgs = "--bin samply";
        };
      in
      {
        checks = {
          # Build the crate as part of `nix flake check` for convenience
          inherit samply;

          # Run clippy (and deny all warnings) on the crate source,
          # again, resuing the dependency artifacts from above.
          #
          # Note that this is done as a separate derivation so that
          # we can block the CI if there are issues here, but not
          # prevent downstream consumers from building our crate by itself.
          cargo-clippy = craneLib.cargoClippy {
            inherit cargoArtifacts src buildInputs;
            cargoClippyExtraArgs = "--all-targets -- --deny warnings";
          };

          cargo-doc = craneLib.cargoDoc {
            inherit cargoArtifacts src buildInputs;
          };

          # Check formatting
          cargo-fmt = craneLib.cargoFmt {
            inherit src;
          };

          # Audit dependencies
          cargo-audit = craneLib.cargoAudit {
            inherit src advisory-db;
          };

          # Run tests with cargo-nextest
          # Consider setting `doCheck = false` on `samply` if you do not want
          # the tests to run twice
          cargo-nextest = craneLib.cargoNextest {
            inherit cargoArtifacts src buildInputs;
            partitions = 1;
            partitionType = "count";
          };
        } // lib.optionalAttrs (system == "x86_64-linux") {
          # NB: cargo-tarpaulin only supports x86_64 systems
          # Check code coverage (note: this will not upload coverage anywhere)
          cargo-coverage = craneLib.cargoTarpaulin {
            inherit cargoArtifacts src;
          };
        };

        packages.default = samply;

        apps.default = flake-utils.lib.mkApp {
          drv = samply;
        };

        overlays = final: prev: {
          inherit samply;
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = builtins.attrValues self.checks;

          packages = with pkgs; [
            rust-toolchain
            rust-analyzer
          ] ++ buildInputs;

        };
      }
    );
}
