// SPDX-License-Identifier: MIT
// CI-only native driver smoke. The parent process supervises this executable;
// polling deadlines do not bound a synchronous Windows/driver call.
#[cfg(windows)]
fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        None => smoke::run(),
        Some("--owner-exit") => smoke::owner_exit(),
        Some("--active-smoke") => smoke::active(false),
        Some("--deactivate-smoke") => smoke::active(true),
        Some(_) => Err(std::io::Error::other("Unexpected CI smoke argument").into()),
    };
    if let Err(error) = result {
        eprintln!("XSCC_IDD_RUNTIME_SMOKE=failed: {error:?}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("XSCC IDD runtime smoke requires an isolated Windows CI VM");
    std::process::exit(1);
}

#[cfg(windows)]
mod smoke {
    use std::{
        error::Error,
        fs,
        io::{self, Write},
        os::windows::process::CommandExt,
        process::Command,
        thread,
        time::{Duration, Instant},
    };
    use xscc_idd_control::{ConnectorId, Mode, Owner};

    type Result<T> = std::result::Result<T, Box<dyn Error>>;

    fn open_ready() -> Result<Owner> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match Owner::open() {
                Ok(owner) => return Ok(owner),
                Err(error) => {
                    let starting = matches!(error.raw_os_error(), Some(2 | 3 | 21))
                        || error
                            .source()
                            .and_then(|cause| cause.downcast_ref::<io::Error>())
                            .is_some_and(|cause| cause.kind() == io::ErrorKind::NotFound);
                    if !starting || Instant::now() >= deadline {
                        return Err(error.into());
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }

    fn inventory(owner: &mut Owner, expected: Option<ConnectorId>) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let list = owner.list()?;
            let query = owner.query()?;
            let matches = match expected {
                Some(id) => list.len() == 1 && list[0].connector == id,
                None => list.is_empty(),
            };
            if matches && query.connected_count as usize == list.len() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other(format!(
                    "inventory did not settle: {list:?}, {query:?}"
                ))
                .into());
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn require_ci() -> Result<()> {
        for (name, expected) in [
            ("GITHUB_ACTIONS", "true"),
            ("RUNNER_ENVIRONMENT", "github-hosted"),
            ("RUNNER_OS", "Windows"),
            ("XSCC_IDD_CI_RUNTIME_SMOKE", "1"),
        ] {
            if std::env::var(name).as_deref() != Ok(expected) {
                return Err(io::Error::other(format!("CI-only guard failed: {name}")).into());
            }
        }
        let event = std::env::var("GITHUB_EVENT_NAME")?;
        let branch_push = event == "push"
            && std::env::var("GITHUB_REF").as_deref()
                == Ok("refs/heads/ci/owned-idd-runtime-20261107");
        if event != "workflow_dispatch" && !branch_push {
            return Err(io::Error::other("not the explicit dispatch or one-off CI branch").into());
        }
        Ok(())
    }

    fn owned(owner: &mut Owner, expected: Option<ConnectorId>) -> Result<()> {
        let ids: Vec<_> = owner
            .owned_list()?
            .into_iter()
            .map(|monitor| monitor.connector)
            .collect();
        if ids != expected.into_iter().collect::<Vec<_>>() {
            return Err(io::Error::other(format!("unexpected lease inventory: {ids:?}")).into());
        }
        Ok(())
    }

    pub(super) fn owner_exit() -> Result<()> {
        require_ci()?;
        let mut owner = open_ready()?;
        let connector = owner.add(
            None,
            &[Mode {
                width: 1280,
                height: 720,
                refresh_rate_units: 1_200_000,
            }],
        )?;
        owned(&mut owner, Some(connector))?;
        println!("phase=child_holds_monitor connector={}", connector.get());
        io::stdout().flush()?;
        // Deliberately bypass Rust Drop: Windows must close this process's file.
        std::process::exit(0);
    }

    pub(super) fn active(deactivate: bool) -> Result<()> {
        require_ci()?;
        let helper = std::env::args()
            .nth(2)
            .ok_or_else(|| io::Error::other("Missing CI helper"))?;
        let directory = std::env::args()
            .nth(3)
            .ok_or_else(|| io::Error::other("Missing dedicated CI directory"))?;
        let ready = std::path::Path::new(&directory).join("active-ready.txt");
        if ready.exists() {
            return Err(io::Error::other("Fresh active helper readiness required").into());
        }
        let root = format!(
            "ROOT\\XSCCIDD\\CI_{}_{}",
            std::env::var("GITHUB_RUN_ID")?,
            std::env::var("GITHUB_RUN_ATTEMPT")?
        );
        let mut owner = open_ready()?;
        inventory(&mut owner, None)?;
        let connector = owner.add(
            None,
            &[Mode {
                width: 1280,
                height: 720,
                refresh_rate_units: 1_200_000,
            }],
        )?;
        let mut child = Command::new(helper)
            .args([
                if deactivate { "--deactivate" } else { "--run" },
                &root,
                &directory,
            ])
            .creation_flags(0x08000000)
            .spawn()?;
        let result = (|| -> Result<()> {
            let deadline = Instant::now() + Duration::from_secs(12);
            loop {
                if let Some(status) = child.try_wait()? {
                    return Err(io::Error::other(format!(
                        "active producer exited before readiness: {status}"
                    ))
                    .into());
                }
                match fs::read_to_string(&ready) {
                    Ok(value) if value == "active-confirmed\n" => break,
                    Ok(_) => return Err(io::Error::other("Invalid active helper readiness").into()),
                    Err(error)
                        if error.kind() == io::ErrorKind::NotFound && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(20))
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            if !deactivate {
                thread::sleep(Duration::from_secs(5));
                if let Some(status) = child.try_wait()? {
                    return Err(io::Error::other(format!(
                        "producer exited before active removal: {status}"
                    ))
                    .into());
                }
                println!(
                    "phase=active_remove_requested connector={}",
                    connector.get()
                );
                owner.remove(connector)?;
                inventory(&mut owner, None)?;
            }
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                match child.try_wait()? {
                    Some(status) if status.success() => break,
                    Some(status) => {
                        return Err(io::Error::other(format!(
                            "active producer/restore failed: {status}"
                        ))
                        .into())
                    }
                    None if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
                    None => {
                        return Err(
                            io::Error::other("active producer did not exit after removal").into(),
                        )
                    }
                }
            }
            if deactivate {
                inventory(&mut owner, Some(connector))?;
                owned(&mut owner, Some(connector))?;
                println!(
                    "phase=os_deactivated_lease_still_owned connector={}",
                    connector.get()
                );
                owner.remove(connector)?;
                inventory(&mut owner, None)?;
            }
            println!("XSCC_IDD_ACTIVE_SMOKE=passed");
            Ok(())
        })();
        if result.is_err() {
            match child.try_wait() {
                Ok(Some(_)) => {}
                status => eprintln!(
                    "own_active_child_state={status:?}; own_child_kill={:?}",
                    child.kill()
                ),
            }
        }
        result
    }

    pub(super) fn run() -> Result<()> {
        require_ci()?;
        let start = Instant::now();
        let mut owner = open_ready()?;
        let mut observer = open_ready()?;
        inventory(&mut observer, None)?;
        owned(&mut owner, None)?;
        owned(&mut observer, None)?;
        println!("phase=empty elapsed_ms={}", start.elapsed().as_millis());
        let modes = [Mode {
            width: 1280,
            height: 720,
            refresh_rate_units: 1_200_000,
        }];
        let connector = owner.add(None, &modes)?;
        inventory(&mut observer, Some(connector))?;
        owned(&mut owner, Some(connector))?;
        owned(&mut observer, None)?;
        println!(
            "phase=add connector={} elapsed_ms={}",
            connector.get(),
            start.elapsed().as_millis()
        );
        match observer.remove(connector) {
            Err(error) if error.raw_os_error() == Some(5) => {}
            Err(error) => return Err(error.into()),
            Ok(()) => {
                return Err(io::Error::other("another lease removed the owner's monitor").into())
            }
        }
        inventory(&mut observer, Some(connector))?;
        owner.remove(connector)?;
        inventory(&mut observer, None)?;
        owned(&mut owner, None)?;
        println!(
            "phase=explicit_remove elapsed_ms={}",
            start.elapsed().as_millis()
        );
        let mut closing_owner = open_ready()?;
        let connector = closing_owner.add(None, &modes)?;
        inventory(&mut observer, Some(connector))?;
        owned(&mut closing_owner, Some(connector))?;
        owned(&mut observer, None)?;
        drop(closing_owner);
        inventory(&mut observer, None)?;
        owned(&mut observer, None)?;
        println!(
            "phase=lease_close elapsed_ms={}",
            start.elapsed().as_millis()
        );
        let mut child = Command::new(std::env::current_exe()?)
            .arg("--owner-exit")
            .creation_flags(0x08000000)
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => break,
                Ok(Some(status)) => {
                    return Err(io::Error::other(format!("owner child failed: {status}")).into())
                }
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
                result => {
                    let kill = child.kill();
                    return Err(io::Error::other(format!(
                        "owner child did not finish: {result:?}; own-child termination: {kill:?}"
                    ))
                    .into());
                }
            }
        }
        inventory(&mut observer, None)?;
        owned(&mut observer, None)?;
        println!(
            "phase=process_exit_no_rust_drop elapsed_ms={}",
            start.elapsed().as_millis()
        );
        println!("XSCC_IDD_RUNTIME_SMOKE=passed");
        Ok(())
    }
}
