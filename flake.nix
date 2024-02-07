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

        crateNameFromCargoToml = craneLib.crateNameFromCargoToml {
          cargoToml = ./samply/Cargo.toml;
        };
        
        src = craneLib.cleanCargoSource ./.;

        buildInputs = with pkgs; [
          # Add additional build inputs here
          openssl
          pkg-config
        ];



        # Build *just* the cargo dependencies, so we can reuse
        # all of that work (e.g. via cachix) when running in CI
        cargoArtifacts = craneLib.buildDepsOnly {
          inherit (crateNameFromCargoToml) pname version;
          inherit src buildInputs;
        };

        # Build the actual crate itself, reusing the dependency
        # artifacts from above.
        samply = craneLib.buildPackage {
          inherit (crateNameFromCargoToml) pname version;
          inherit cargoArtifacts src buildInputs;
          cargoExtraArgs = "--bin samply";
        };
      in
      {
        checks = {
          # Build the crate as part of `nix flake check` for convenience
          inherit samply;
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
