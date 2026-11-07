// SPDX-License-Identifier: MIT
// CI-only native driver smoke. The parent process supervises this executable;
// polling deadlines do not bound a synchronous Windows/driver call.
#[cfg(windows)]
fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        None => smoke::run(),
        Some("--owner-exit") => smoke::owner_exit(),
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
