// font-kit/src/matching.rs
//
// Copyright © 2018 The Pathfinder Project Developers.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Determines the closest font matching a description per the CSS Fonts Level 3 specification.

use crate::{error::SelectionError, properties::Properties};

/// Returns the first candidate that matches the requested font properties.
pub fn find_best_match(
    candidates: &[Properties],
    query: &Properties,
) -> Result<usize, SelectionError> {
    font_style_matching::find_best_match(candidates, query).ok_or(SelectionError::NotFound)
}
