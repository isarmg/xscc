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

use float_ord::FloatOrd;

use crate::properties::{Properties, Stretch, Style, Weight};

pub mod properties;

/// Returns the first matching candidate per the [CSS Fonts Level 3 specification].
///
/// [CSS Fonts Level 3 specification]: https://drafts.csswg.org/css-fonts-3/#font-style-matching
pub fn find_best_match(candidates: &[Properties], query: &Properties) -> Option<usize> {
    // Step 4.
    let mut matching_set: Vec<usize> = (0..candidates.len()).collect();
    if matching_set.is_empty() {
        return None;
    }

    // Step 4a (`font-stretch`).
    let matching_stretch = if matching_set
        .iter()
        .any(|&index| candidates[index].stretch == query.stretch)
    {
        // Exact match.
        query.stretch
    } else if query.stretch <= Stretch::NORMAL {
        // Closest width, first checking narrower values and then wider values.
        match matching_set
            .iter()
            .filter(|&&index| candidates[index].stretch < query.stretch)
            .min_by_key(|&&index| FloatOrd(query.stretch.0 - candidates[index].stretch.0))
        {
            Some(&matching_index) => candidates[matching_index].stretch,
            None => {
                let matching_index = *matching_set.iter().min_by_key(|&&index| {
                    FloatOrd(candidates[index].stretch.0 - query.stretch.0)
                })?;
                candidates[matching_index].stretch
            }
        }
    } else {
        // Closest width, first checking wider values and then narrower values.
        match matching_set
            .iter()
            .filter(|&&index| candidates[index].stretch > query.stretch)
            .min_by_key(|&&index| FloatOrd(candidates[index].stretch.0 - query.stretch.0))
        {
            Some(&matching_index) => candidates[matching_index].stretch,
            None => {
                let matching_index = *matching_set.iter().min_by_key(|&&index| {
                    FloatOrd(query.stretch.0 - candidates[index].stretch.0)
                })?;
                candidates[matching_index].stretch
            }
        }
    };
    matching_set.retain(|&index| candidates[index].stretch == matching_stretch);

    // Step 4b (`font-style`).
    let style_preference = match query.style {
        Style::Italic => [Style::Italic, Style::Oblique, Style::Normal],
        Style::Oblique => [Style::Oblique, Style::Italic, Style::Normal],
        Style::Normal => [Style::Normal, Style::Oblique, Style::Italic],
    };
    let matching_style = *style_preference.iter().find(|&query_style| {
        matching_set
            .iter()
            .any(|&index| candidates[index].style == *query_style)
    })?;
    matching_set.retain(|&index| candidates[index].style == matching_style);

    // Step 4c (`font-weight`).
    //
    // The spec doesn't say what to do if the weight is between 400 and 500 exclusive, so we
    // just use 450 as the cutoff.
    let matching_weight = if matching_set
        .iter()
        .any(|&index| candidates[index].weight == query.weight)
    {
        query.weight
    } else if query.weight >= Weight(400.0)
        && query.weight < Weight(450.0)
        && matching_set
            .iter()
            .any(|&index| candidates[index].weight == Weight(500.0))
    {
        // Check 500 first.
        Weight(500.0)
    } else if query.weight >= Weight(450.0)
        && query.weight <= Weight(500.0)
        && matching_set
            .iter()
            .any(|&index| candidates[index].weight == Weight(400.0))
    {
        // Check 400 first.
        Weight(400.0)
    } else if query.weight <= Weight(500.0) {
        // Closest weight, first checking thinner values and then fatter ones.
        match matching_set
            .iter()
            .filter(|&&index| candidates[index].weight <= query.weight)
            .min_by_key(|&&index| FloatOrd(query.weight.0 - candidates[index].weight.0))
        {
            Some(&matching_index) => candidates[matching_index].weight,
            None => {
                let matching_index = *matching_set
                    .iter()
                    .min_by_key(|&&index| FloatOrd(candidates[index].weight.0 - query.weight.0))?;
                candidates[matching_index].weight
            }
        }
    } else {
        // Closest weight, first checking fatter values and then thinner ones.
        match matching_set
            .iter()
            .filter(|&&index| candidates[index].weight >= query.weight)
            .min_by_key(|&&index| FloatOrd(candidates[index].weight.0 - query.weight.0))
        {
            Some(&matching_index) => candidates[matching_index].weight,
            None => {
                let matching_index = *matching_set
                    .iter()
                    .min_by_key(|&&index| FloatOrd(query.weight.0 - candidates[index].weight.0))?;
                candidates[matching_index].weight
            }
        }
    };
    matching_set.retain(|&index| candidates[index].weight == matching_weight);

    // Step 4d concerns `font-size`, but fonts in `font-kit` are unsized, so we ignore that.

    // Return the result.
    matching_set.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::{find_best_match, Properties, Stretch, Style, Weight};

    #[test]
    fn css_width_precedes_style_and_weight() {
        let query = Properties {
            style: Style::Italic,
            weight: Weight::MEDIUM,
            ..Properties::default()
        };
        let candidates = [
            Properties {
                stretch: Stretch::CONDENSED,
                ..query
            },
            Properties {
                style: Style::Oblique,
                weight: Weight::MEDIUM,
                ..Properties::default()
            },
            Properties {
                style: Style::Italic,
                weight: Weight::NORMAL,
                ..Properties::default()
            },
        ];

        assert_eq!(find_best_match(&candidates, &query), Some(2));
    }

    #[test]
    fn rejects_a_candidate_with_an_invalid_width() {
        let invalid = Properties {
            stretch: Stretch(f32::NAN),
            ..Properties::default()
        };
        assert_eq!(find_best_match(&[invalid], &Properties::default()), None);
    }

    #[test]
    fn undefined_width_class_uses_the_normal_width() {
        assert_eq!(Stretch::from_width_class(0), Stretch::NORMAL);
        assert_eq!(Stretch::from_width_class(5), Stretch::NORMAL);
        assert_eq!(Stretch::from_width_class(9), Stretch::ULTRA_EXPANDED);
        assert_eq!(Stretch::from_width_class(10), Stretch::NORMAL);
    }
}
