{ pkgs, deps }: {
  prepare_class = let
    app = pkgs.writeShellApplication {
      name = "prepare-class";
      runtimeInputs = [];
      text = ''
        if ! [ -z "$(git diff)" ]; then
          echo "Staging changes, commit then execute again"
          exit 1
        fi

        git checkout -B class
        rm -rf .forgejo
        rm flake.*
        rm economy.ods
        rm example/bigtest.sh
        rm -rf .swagger
        git add .
        git commit -m "Preparing project for class"
        git push -f -u origin class
      '';
    };
  in "${app}/bin/prepare-class";

  unit_tests = let
    app = pkgs.writeShellApplication {
      name = "unit-tests";
      runtimeInputs = deps;
      text = ''
        export CARGO_HOME=$PWD/.cargohome
        cargo test
      '';
    };
  in "${app}/bin/unit-tests";

  functional_tests = let
    python = pkgs.python312.withPackages (p: [ p.requests ]);
    app = pkgs.writeShellApplication {
      name = "functionnal-tests";
      runtimeInputs = deps ++ [ python ];
      text = ''
        export CARGO_HOME=$PWD/.cargohome
        export RUST_BACKTRACE=full
        export RUST_LOG=debug
        mkdir -p target
        cargo build --target-dir ./target/functests/ --features testing
        rm -f /tmp/simeis_logs
        ./target/functests/debug/simeis-server 1>/tmp/simeis_logs 2>&1 &
        sleep 5

        if [ -z "$(jobs -r)" ]; then
          echo "!!! Failed to start the server";
          cat /tmp/simeis_logs
          exit 1;
        fi

        if ! python3 .forgejo/functests.py 127.0.0.1 9345 "$@"; then
          echo "Some tests failed"
          kill "$(jobs -p)"

          echo "Server logs:"
          tail -n 20 /tmp/simeis_logs
          exit 1;
        fi

        kill "$(jobs -p)"
        echo "[*] Finished"
      '';
    };
  in "${app}/bin/functionnal-tests";

  check_rust_code = let
    allIgnoredCVE = [
      "RUSTSEC-2025-0047"
    ];
    allIgnored = builtins.concatStringsSep " " (builtins.map (s: "--ignore ${s}") allIgnoredCVE);
    app = pkgs.writeShellApplication {
      name = "rust-check-code";
      runtimeInputs = deps ++ [ pkgs.gcc ];

      text = ''
        set -e
        export CARGO_HOME=$PWD/.cargohome
        cargo check
        cargo clippy --no-deps --frozen -- -D warnings
        cargo fmt --check
        cargo audit ${allIgnored}
        if ! [ -d supply-chain ]; then
          cargo vet init
        fi
        cargo vet
      '';
    };
  in "${app}/bin/rust-check-code";

  check_todos = let
    ignored=builtins.concatStringsSep "|" [
      ".git"
      ".forgejo/scripts.nix"
      ".forgejo/workflows"
      ".swagger"
      "TODO.md"
    ];

    app = pkgs.writeShellApplication {
      name = "check-todos";
      runtimeInputs = [ pkgs.ripgrep pkgs.jq ];
      text = ''
        set -e
        ERRCODE=0
        CURL_OPTS="-s"
        git fetch origin "$GITHUB_BASE_REF"
        echo ""

        set +e
        FILES_TO_CHECK=$( rg -l "TODO" | rg -v "${ignored}" | tr '\n' ' ')
        git diff "origin/$GITHUB_BASE_REF" -- "$FILES_TO_CHECK" \
          | rg -v "^-" \
          | tee todos_to_check

        rg -o "\(#\d+\)" todos_to_check \
          | rg -o "\d+" \
          | sort -u -n \
          | tee issues_to_check

        while read -r ISSUE; do
          if ! curl "$CURL_OPTS" \
            "$GITHUB_SERVER_URL/api/v1/repos/$GITHUB_REPOSITORY/issues/$ISSUE" \
            | jq ".state" \
            | rg "\"open\"" \
            2>/dev/null 1>/dev/null
          then
            echo "[!] Issue $ISSUE is closed or missing"
            ERRCODE=1
          fi
        done < issues_to_check
        echo ""

        if echo "$FILES_TO_CHECK" | xargs -n 1 rg "TODO" | rg -v "\(#\d+\)"; then
          echo "[!] Some TODOs are not linked to an existing issue"
          ERRCODE=1
        fi
        echo ""

        exit $ERRCODE
      '';
    };
    in "${app}/bin/check-todos";

    apiSanity = let
      app = pkgs.writeShellApplication {
        name = "apiSanity";
        runtimeInputs = [ pkgs.ripgrep ];
        text = ''
          export CARGO_HOME=$PWD/.cargohome
        export RUST_BACKTRACE=full
          mkdir -p target
          cargo build --target-dir ./target/apisanity/ --features testing
          rm -f /tmp/simeis_logs
          ./target/apisanity/debug/simeis-server 1>/tmp/simeis_logs 2>&1 &
          sleep 5
          if [ -z "$(jobs -r)" ]; then
            echo "!!! Failed to start the server";
            cat /tmp/simeis_logs
            exit 1;
          fi
          if ! python3 .forgejo/generate_swagger.py 127.0.0.1 9345 "$@"; then
            echo "Error while generating swagger file"
            kill "$(jobs -p)"
            exit 1;
          fi

          kill "$(jobs -p)"
          echo "[*] Finished"
        '';
      };
    in "${app}/bin/apiSanity";
  }
