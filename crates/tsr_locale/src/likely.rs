//! Pinned CLDR likely-subtag expansion, x/text internal/language/match.go.
use crate::{data, lookup, Locale};
fn contains(region: u16, child: u16) -> bool {
    if region == child {
        return true;
    }
    let group = usize::from(data::REGION_INCLUSION[usize::from(region)]);
    if group >= data::REGION_CONTAINMENT.len() {
        return false;
    }
    let mask = data::REGION_CONTAINMENT[group];
    let sub = usize::from(data::REGION_INCLUSION[usize::from(child)]);
    let bits = data::REGION_INCLUSION_BITS[sub];
    if sub >= data::REGION_CONTAINMENT.len() {
        bits & mask != 0
    } else {
        bits & !mask == 0
    }
}
impl Locale {
    fn set_region(&mut self, region: u16) {
        if self.region == 0 || contains(self.region, region) {
            self.region = region;
        }
    }
    fn set_script(&mut self, script: u16) {
        if self.script == 0 {
            self.script = script;
        }
    }
    fn set_language(&mut self, language: u16) {
        if self.language == 0 {
            self.language = language;
        }
    }
    fn specialize_region(&mut self) {
        let group = usize::from(data::REGION_INCLUSION[usize::from(self.region)]);
        if let Some(&[language, region, script]) = data::LIKELY_REGION_GROUP.get(group) {
            if language == self.language && script == self.script {
                self.region = region;
            }
        }
    }
    pub(crate) fn maximize(&mut self) {
        if self.private {
            return;
        }
        if self.script != 0 && self.region != 0 {
            if self.language != 0 {
                self.specialize_region();
                return;
            }
            let row = data::LIKELY_REGION[usize::from(self.region)];
            let list = if row[2] & 1 != 0 {
                &data::LIKELY_REGION_LIST[usize::from(row[0])..usize::from(row[0] + row[1])]
            } else {
                std::slice::from_ref(&row)
            };
            if let Some(row) = list.iter().find(|row| row[1] == self.script) {
                self.set_language(row[0]);
                return;
            }
        }
        if self.language != 0 {
            if let Some(row) = data::LIKELY_LANG
                .get(usize::from(self.language))
                .filter(|row| row[2] & 1 != 0)
            {
                let list =
                    &data::LIKELY_LANG_LIST[usize::from(row[0])..usize::from(row[0] + row[1])];
                if self.script != 0 {
                    if let Some(row) = list
                        .iter()
                        .find(|row| row[1] == self.script && row[2] & 2 != 0)
                    {
                        self.set_region(row[0]);
                        return;
                    }
                } else if self.region != 0 {
                    let mut candidate = self.clone();
                    let mut count = 0;
                    let mut good = true;
                    for row in list {
                        if row[2] & 2 == 0 && contains(self.region, row[0]) {
                            candidate.region = row[0];
                            candidate.set_script(row[1]);
                            good &= candidate.script == row[1];
                            count += 1;
                        }
                    }
                    if count == 1 {
                        *self = candidate;
                        return;
                    }
                    if good {
                        self.script = candidate.script;
                    }
                }
            }
        } else {
            if self.script != 0 {
                let row = data::LIKELY_SCRIPT[usize::from(self.script)];
                if row[1] != 0 {
                    self.set_region(row[1]);
                    self.set_language(row[0]);
                    return;
                }
            }
            if self.region != 0 {
                let group = usize::from(data::REGION_INCLUSION[usize::from(self.region)]);
                if let Some(&[language, region, script]) = data::LIKELY_REGION_GROUP.get(group) {
                    if region != 0 {
                        self.set_language(language);
                        self.set_script(script);
                        self.region = region;
                    }
                } else {
                    let mut row = data::LIKELY_REGION[usize::from(self.region)];
                    if row[2] & 1 != 0 {
                        row = data::LIKELY_REGION_LIST[usize::from(row[0])];
                    }
                    if row[1] != 0 && row[2] != 2 {
                        self.set_language(row[0]);
                        self.set_script(row[1]);
                        return;
                    }
                }
            }
        }
        if let Some(mut row) = data::LIKELY_LANG.get(usize::from(self.language)).copied() {
            if row[2] & 1 != 0 {
                row = data::LIKELY_LANG_LIST[usize::from(row[0])];
            }
            if row[0] != 0 {
                self.set_script(row[1]);
                self.set_region(row[0]);
            }
            self.specialize_region();
            self.set_language(lookup(data::LANGUAGES, "en").unwrap());
        }
    }
}
