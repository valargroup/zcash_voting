use clap::Parser;
use recovery_conformance::stage_config::StageDeployment;
use stage_bench::helper_fleet::HelperFleetArgs;

#[derive(Parser)]
struct Arguments {
    #[command(flatten)]
    helpers: HelperFleetArgs,
}

fn deployment() -> StageDeployment {
    serde_json::from_value(serde_json::json!({
        "config_version": 1,
        "vote_servers": [
            {"url": "https://stage.vote-chain-primary.valargroup.org"},
            {"url": "https://stage.vote-chain-secondary.valargroup.org"}
        ],
        "pir_endpoints": [{"url": "https://stage.pir.valargroup.org"}],
        "pir_layout": {"pir_depth": 19, "tier0_layers": 12, "tier1_layers": 7, "poly_len": 4096},
        "supported_versions": {}
    }))
    .unwrap()
}

#[test]
fn default_and_explicit_two_helpers_use_both_real_servers_without_rewriting() {
    for args in [vec!["bench"], vec!["bench", "--helpers", "2"]] {
        let deployment = deployment();
        let selected = Arguments::try_parse_from(args)
            .unwrap()
            .helpers
            .resolve(&deployment)
            .unwrap();
        assert_eq!(
            selected.endpoints.helper_urls,
            deployment.vote_server_urls()
        );
        assert_eq!(
            selected.endpoints.vote_servers,
            deployment.vote_server_urls()
        );
        assert_eq!(selected.endpoints.pir_urls, deployment.pir_urls());
        assert!(selected.fleet.configured_urls().is_empty());
        for url in &selected.endpoints.helper_urls {
            assert!(selected
                .fleet
                .resolve(&format!("{url}/shielded-vote/v1/shares"))
                .is_none());
        }
    }
}

#[test]
fn primary_only_is_explicit_and_does_not_remove_chain_failover() {
    let deployment = deployment();
    let selected = Arguments::try_parse_from(["bench", "--helpers", "1"])
        .unwrap()
        .helpers
        .resolve(&deployment)
        .unwrap();
    assert_eq!(
        selected.endpoints.helper_urls,
        deployment.vote_server_urls()[..1]
    );
    assert_eq!(selected.endpoints.vote_servers.len(), 2);
    assert!(selected.fleet.configured_urls().is_empty());
}

#[test]
fn synthetic_fanout_requires_explicit_mode_and_still_routes_to_primary() {
    for count in ["2", "10"] {
        let deployment = deployment();
        let selected = Arguments::try_parse_from(["bench", "--synthetic-helpers", count])
            .unwrap()
            .helpers
            .resolve(&deployment)
            .unwrap();
        assert_eq!(
            selected.endpoints.helper_urls.len(),
            count.parse::<usize>().unwrap()
        );
        assert_eq!(
            selected.endpoints.helper_urls,
            selected.fleet.configured_urls()
        );
        assert_eq!(selected.fleet.backend, deployment.vote_server_urls()[0]);
        for url in &selected.endpoints.helper_urls {
            assert!(selected
                .fleet
                .resolve(&format!("{url}/shielded-vote/v1/shares"))
                .is_some());
        }
        assert_eq!(
            selected.endpoints.vote_servers,
            deployment.vote_server_urls()
        );
    }
}

#[test]
fn invalid_counts_and_conflicting_modes_fail_during_argument_parsing() {
    for args in [
        vec!["bench", "--helpers", "0"],
        vec!["bench", "--helpers", "3"],
        vec!["bench", "--synthetic-helpers", "1"],
        vec!["bench", "--synthetic-helpers", "11"],
        vec!["bench", "--helpers", "2", "--synthetic-helpers", "2"],
    ] {
        assert!(Arguments::try_parse_from(args).is_err());
    }
}

#[test]
fn run_and_preflight_reject_invalid_helper_flags_before_resolving_credentials() {
    for command in ["run", "preflight"] {
        for flags in [
            vec!["--helpers", "3"],
            vec!["--helpers", "2", "--synthetic-helpers", "2"],
        ] {
            let output = std::process::Command::new(env!("CARGO_BIN_EXE_stage-bench"))
                .arg(command)
                .args(flags)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(2));
            assert!(String::from_utf8_lossy(&output.stderr).contains("--helpers"));
        }
        let help = std::process::Command::new(env!("CARGO_BIN_EXE_stage-bench"))
            .args([command, "--help"])
            .output()
            .unwrap();
        assert!(help.status.success());
        let help = String::from_utf8_lossy(&help.stdout);
        assert!(help.contains("--helpers"));
        assert!(help.contains("--synthetic-helpers"));
    }
}

#[test]
fn missing_or_duplicate_secondary_never_silently_becomes_a_one_helper_run() {
    let args = Arguments::try_parse_from(["bench"]).unwrap();
    let mut deployment = deployment();
    deployment.vote_servers.truncate(1);
    assert!(args
        .helpers
        .resolve(&deployment)
        .unwrap_err()
        .to_string()
        .contains("publishes only 1"));
    deployment
        .vote_servers
        .push(deployment.vote_servers[0].clone());
    assert!(args
        .helpers
        .resolve(&deployment)
        .unwrap_err()
        .to_string()
        .contains("duplicate"));
    deployment.vote_servers[1].url = "not a URL".into();
    assert!(args.helpers.resolve(&deployment).is_err());
    deployment.vote_servers.clear();
    let synthetic = Arguments::try_parse_from(["bench", "--synthetic-helpers", "2"]).unwrap();
    assert!(synthetic.helpers.resolve(&deployment).is_err());
}
