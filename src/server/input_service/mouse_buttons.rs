use std::{collections::HashMap, time::Instant};

#[derive(Default)]
pub(super) struct PressedMouseButtons {
    owners: HashMap<(i32, i32), Instant>,
}

impl PressedMouseButtons {
    pub(super) fn press<E>(
        &mut self,
        conn: i32,
        button: i32,
        inject: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E> {
        if !self.owners.keys().any(|(_, held)| *held == button) {
            inject()?;
        }
        self.owners.insert((conn, button), Instant::now());
        Ok(())
    }

    pub(super) fn release_matching(
        &mut self,
        mut matches: impl FnMut(i32, i32, Instant) -> bool,
        mut inject: impl FnMut(i32),
    ) {
        let mut released = Vec::new();
        self.owners.retain(|&(conn, button), &mut at| {
            if matches(conn, button, at) {
                if !released.contains(&button) {
                    released.push(button);
                }
                false
            } else {
                true
            }
        });
        for button in released {
            if !self.owners.keys().any(|(_, held)| *held == button) {
                inject(button);
            }
        }
    }
}

#[cfg(test)]
mod mouse_button_tests {
    use super::*;

    #[test]
    fn queued_downs_then_disconnect_release_only_the_last_owner() {
        let mut buttons = PressedMouseButtons::default();
        // Connection's input FIFO drains accepted downs before its cleanup ups.
        let (tx, rx) = std::sync::mpsc::channel();
        for event in [
            (1, true),
            (1, true),
            (2, true),
            (1, false),
            (3, false),
            (2, false),
            (2, false),
        ] {
            tx.send(event).unwrap();
        }
        drop(tx);
        let mut injected = Vec::new();
        for (conn, down) in rx {
            if down {
                buttons
                    .press(conn, 1, || {
                        injected.push((true, 1));
                        Ok::<_, ()>(())
                    })
                    .unwrap();
            } else {
                buttons
                    .release_matching(|owner, _, _| owner == conn, |b| injected.push((false, b)));
            }
            if conn == 1 || conn == 3 {
                assert_eq!(injected, [(true, 1)]);
            }
        }
        assert_eq!(injected, [(true, 1), (false, 1)]);
        buttons
            .press(1, 1, || {
                injected.push((true, 1));
                Ok::<_, ()>(())
            })
            .unwrap();
        assert_eq!(injected, [(true, 1), (false, 1), (true, 1)]);
    }

    #[test]
    fn failed_down_does_not_own_a_button_or_suppress_the_next_down() {
        let mut buttons = PressedMouseButtons::default();
        assert_eq!(
            buttons.press(1, 1, || Err("injection failed")),
            Err("injection failed")
        );
        buttons.release_matching(|_, _, _| true, |_| panic!("no successful down"));
        let mut downs = 0;
        buttons
            .press(2, 1, || {
                downs += 1;
                Ok::<_, ()>(())
            })
            .unwrap();
        assert_eq!(downs, 1);
    }

    #[test]
    fn timeout_and_revocation_release_distinct_buttons_without_releasing_another_owner() {
        let mut buttons = PressedMouseButtons::default();
        buttons.press(1, 1, || Ok::<_, ()>(())).unwrap();
        buttons.press(1, 2, || Ok::<_, ()>(())).unwrap();
        let cutoff = Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(1));
        buttons.press(2, 1, || Ok::<_, ()>(())).unwrap();
        let mut ups = Vec::new();
        buttons.release_matching(|_, _, at| at <= cutoff, |b| ups.push(b));
        assert_eq!(ups, [2]);
        buttons.release_matching(|conn, _, _| conn == 2, |b| ups.push(b));
        assert_eq!(ups, [2, 1]);
    }
}
