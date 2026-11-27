use super::*;

fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from([&["horizon-launcher", "sources"], args].concat())
}

#[test]
fn sources_parse_each_action() {
    assert!(matches!(
        parse(&[]).unwrap().command,
        Some(Cmd::Sources { action: None })
    ));
    assert!(matches!(
        parse(&["list"]).unwrap().command,
        Some(Cmd::Sources {
            action: Some(SourcesCmd::List)
        })
    ));
    assert!(matches!(
        parse(&["add", "owner/repo", "--no-auto-update"])
            .unwrap()
            .command,
        Some(Cmd::Sources {
            action: Some(SourcesCmd::Add {
                no_auto_update: true,
                ..
            })
        })
    ));
    assert!(matches!(
        parse(&["remove", "owner/repo"]).unwrap().command,
        Some(Cmd::Sources {
            action: Some(SourcesCmd::Remove { .. })
        })
    ));
    assert!(matches!(
        parse(&["update"]).unwrap().command,
        Some(Cmd::Sources {
            action: Some(SourcesCmd::Update { repo: None })
        })
    ));
}

#[test]
fn sources_auto_takes_on_or_off_only() {
    assert!(matches!(
        parse(&["auto", "owner/repo", "on"]).unwrap().command,
        Some(Cmd::Sources {
            action: Some(SourcesCmd::Auto { .. })
        })
    ));
    assert_eq!(on_off("on"), Ok(true));
    assert_eq!(on_off("off"), Ok(false));
    assert!(on_off("maybe").is_err());
}
