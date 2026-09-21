//! Single requested tag against the pinned diagnostic roster. This ports
//! x/text/language bestMatch.update; candidate expansion remains generated data.
use crate::{data, Locale};
fn region_distance(a: u16, b: u16, script: u16, language: u16) -> (u16, bool) {
    let a = u32::from(data::REGION_GROUPS[usize::from(a)]) << 1;
    let b = u32::from(data::REGION_GROUPS[usize::from(b)]) << 1;
    for &[lang, scr, group, distance] in data::MATCH_REGION {
        if lang == language && (scr == 0 || scr == script) {
            let bit = 1u32 << (group & !0x80);
            if (group & 0x80 == 0 && a & b & bit != 0) || (group & 0x80 != 0 && (a | b) & bit == 0)
            {
                return (distance, distance == 4);
            }
        }
    }
    (4, true)
}
pub(crate) fn select(wanted: &Locale) -> Option<usize> {
    let mut want = wanted.clone();
    let mut max = want.clone();
    if want.language != 0 {
        max.canonicalize(true);
        want.region = max.region;
    } else if want.script == 0 && want.region == 0 {
        return None;
    }
    max.maximize();
    let base = if want.language == 0 {
        max.language
    } else {
        want.language
    };
    let index = data::CANDIDATES
        .binary_search_by_key(&base, |&(id, _)| id)
        .ok()?;
    let mut best = None;
    let mut best_conf = 0;
    let mut best_rank = (false, false, 0, false, false);
    let mut same_group = false;
    let has_variants = want.private
        || want
            .tail
            .split('-')
            .take_while(|part| part.len() != 1)
            .any(|part| !part.is_empty());
    for &[index, lang, script, region, max_script, max_region, alt_script, mut confidence, _] in
        data::CANDIDATES[index].1
    {
        if confidence < best_conf {
            continue;
        }
        if same_group && !region_distance(max.region, max_region, max_script, want.language).1 {
            continue;
        }
        let equal_rest = script == want.script && region == want.region && !has_variants;
        if !equal_rest {
            if max_script != max.script {
                if best_conf > 1 || alt_script != max.script {
                    continue;
                }
                confidence = 1;
            } else if max_region != max.region {
                confidence = confidence.min(2);
            }
        }
        if confidence < best_conf {
            continue;
        }
        let (distance, same) = region_distance(max_region, max.region, max.script, want.language);
        let paradigm = data::PARADIGMS
            .iter()
            .any(|row| row[0] == want.language && (row[1] == max_region || row[2] == max_region));
        let rank = (
            lang == want.language && want.language != 0,
            region == want.region && want.region != 0,
            u16::MAX - distance,
            paradigm,
            script == want.script && want.script != 0,
        );
        if confidence > best_conf || rank > best_rank {
            best = Some(usize::from(index));
            best_conf = confidence;
            best_rank = rank;
            same_group = same;
        }
        // The shipped roster has no equal-max continuation candidates; an
        // exact match ends the pinned search just as it does upstream.
        if best_conf == 3 {
            break;
        }
    }
    best
}
