//! Pure planning for one desktop transition before an owned adapter is stopped.
use std::collections::BTreeMap;

pub(super) struct DesktopPath {
    pub target: String,
    pub source: usize,
    pub owned: bool,
    pub position: (i32, i32),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct RemovalPlan {
    pub positions: Vec<(usize, (i32, i32))>,
}

pub(super) fn plan(
    paths: &[DesktopPath],
    primary: Option<&str>,
) -> Result<RemovalPlan, &'static str> {
    let retained: Vec<_> = paths.iter().filter(|path| !path.owned).collect();
    let selected = match primary {
        Some(target) => retained
            .iter()
            .find(|path| path.target.eq_ignore_ascii_case(target)),
        None => retained
            .iter()
            .find(|path| path.position == (0, 0))
            .or_else(|| retained.first()),
    }
    .ok_or("No surviving primary display is available; the adapter was preserved")?;
    let (x, y) = selected.position;
    let mut positions = BTreeMap::new();
    for path in retained {
        let position = (
            path.position
                .0
                .checked_sub(x)
                .ok_or("Desktop X translation overflow")?,
            path.position
                .1
                .checked_sub(y)
                .ok_or("Desktop Y translation overflow")?,
        );
        if positions
            .insert(path.source, position)
            .is_some_and(|previous| previous != position)
        {
            return Err("Cloned display paths disagree about their source position");
        }
    }
    Ok(RemovalPlan {
        positions: positions.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn path(target: &str, source: usize, owned: bool, x: i32, y: i32) -> DesktopPath {
        DesktopPath {
            target: target.into(),
            source,
            owned,
            position: (x, y),
        }
    }

    #[test]
    fn removes_only_owned_sources_and_restores_surviving_geometry_in_one_plan() {
        let paths = [
            path("physical", 4, false, -1920, 0),
            path("owned", 7, true, 0, 0),
            path("other-app", 8, false, -3200, -80),
            path("physical-clone", 4, false, -1920, 0),
        ];
        assert_eq!(
            plan(&paths, Some("PHYSICAL")).unwrap().positions,
            vec![(4, (0, 0)), (8, (-1280, -80))]
        );
    }

    #[test]
    fn never_removes_the_only_display_or_continues_after_primary_loss_or_overflow() {
        assert!(plan(&[path("owned", 0, true, 0, 0)], None).is_err());
        assert!(plan(&[path("physical", 0, false, 0, 0)], Some("unplugged")).is_err());
        assert!(plan(
            &[
                path("a", 0, false, i32::MIN, 0),
                path("b", 1, false, i32::MAX, 0)
            ],
            Some("b")
        )
        .is_err());
    }

    #[test]
    fn retains_current_non_owned_primary_and_rejects_inconsistent_clones() {
        let paths = [
            path("left", 2, false, -1280, 0),
            path("primary", 3, false, 0, 0),
        ];
        assert_eq!(
            plan(&paths, None).unwrap().positions,
            vec![(2, (-1280, 0)), (3, (0, 0))]
        );
        assert!(plan(
            &[path("a", 0, false, 0, 0), path("b", 0, false, 1, 0)],
            None
        )
        .is_err());
    }
}
