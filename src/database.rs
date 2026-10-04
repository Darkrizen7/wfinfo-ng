use std::{collections::HashMap, fs::read_to_string, path::Path};

use levenshtein::levenshtein;
use log::warn;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    statistics::{self, Bucket},
    wfinfo_data::{
        item_data::{EquipmentType, FilteredItems, Refinement, Relic, Relics},
        price_data::PriceItem,
    },
};

#[derive(Clone, Debug, Deserialize)]
pub struct Database {
    items: Vec<Item>,
    pub relics: Relics,
}

#[derive(Clone, Copy, Debug)]
pub struct RefinementValue {
    pub refinement: Refinement,
    /// Expected platinum in a public squad
    pub platinum: f32,
    /// Platinum gained per Void Trace compared to opening the relic intact
    pub platinum_per_trace: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct RelicAdvice {
    /// One entry per refinement, from intact to radiant
    pub values: [RefinementValue; 4],
    /// Minimum platinum per Void Trace for a refinement to be worth it
    pub trace_threshold: f32,
}

impl RelicAdvice {
    pub fn value(&self, refinement: Refinement) -> RefinementValue {
        self.values[Refinement::ALL
            .iter()
            .position(|r| *r == refinement)
            .unwrap()]
    }

    /// Best refinement to reach from `current`, with the platinum gained per trace spent,
    /// or `None` if no refinement is worth its traces
    pub fn recommendation(&self, current: Refinement) -> Option<(Refinement, f32)> {
        let current = self.value(current);
        let per_trace = |value: &RefinementValue| {
            (value.platinum - current.platinum)
                / (value.refinement.trace_cost() - current.refinement.trace_cost()) as f32
        };
        self.values
            .iter()
            .filter(|value| value.refinement.trace_cost() > current.refinement.trace_cost())
            .max_by(|a, b| {
                let score = |value: &RefinementValue| {
                    value.platinum - self.trace_threshold * value.refinement.trace_cost() as f32
                };
                score(a).total_cmp(&score(b))
            })
            .filter(|value| per_trace(value) > self.trace_threshold)
            .map(|value| (value.refinement, per_trace(value)))
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Item {
    pub name: String,
    pub drop_name: String,
    pub platinum: f32,
    pub ducats: usize,
}

impl Database {
    pub fn load_from_file(prices: Option<&Path>, filtered_items: Option<&Path>) -> Database {
        // download file from: https://api.warframestat.us/wfinfo/prices
        let text = read_to_string(prices.unwrap_or_else(|| Path::new("prices.json"))).unwrap();
        let price_list: Vec<PriceItem> = serde_json::from_str(&text).unwrap();
        let price_table: HashMap<String, f32> = price_list
            .into_iter()
            .map(|item| (item.name, item.custom_avg))
            .collect();

        let text =
            read_to_string(filtered_items.unwrap_or_else(|| Path::new("filtered_items.json")))
                .unwrap();
        let mut json = serde_json::from_str(&text).unwrap();

        remove_empty_relics_from_json(&mut json);

        let filtered_items: FilteredItems = serde_json::from_value(json).unwrap();

        let mut items: Vec<_> = filtered_items
            .eqmt
            .iter()
            .flat_map(|(_name, equipment_item)| {
                equipment_item
                    .parts
                    .iter()
                    .filter_map(|(name, ducat_item)| {
                        let item_is_part = name.ends_with("Systems")
                            || name.ends_with("Neuroptics")
                            || name.ends_with("Chassis")
                            || name.ends_with("Harness")
                            || name.ends_with("Wings");
                        let drop_name = match equipment_item.item_type {
                            EquipmentType::Warframes | EquipmentType::Archwing
                                if item_is_part && !name.ends_with("Blueprint") =>
                            {
                                name.to_owned() + " Blueprint"
                            }
                            _ => name.to_owned(),
                        };
                        let platinum = *match price_table
                            .get(name)
                            .or_else(|| price_table.get(&format!("{name} Blueprint")))
                        {
                            Some(plat) => plat,
                            None => {
                                println!("Failed to find price for item: {name}");
                                return None;
                            }
                        };
                        let ducats = ducat_item.ducats;

                        Some(Item {
                            name: name.to_string(),
                            drop_name,
                            platinum,
                            ducats,
                        })
                    })
            })
            .chain(filtered_items.ignored_items.keys().map(|name| Item {
                name: name.to_owned(),
                drop_name: name.to_owned(),
                platinum: 0.0,
                ducats: 0,
            }))
            .collect();

        if let Some(item) = items.iter_mut().find(|item| item.name == "Forma Blueprint") {
            item.platinum = 35.0 / 3.0;
        };

        let relics = filtered_items.relics;

        Database { items, relics }
    }

    pub fn find_item(&self, needle: &str, threshold: Option<usize>) -> Option<&Item> {
        let best_match = self
            .items
            .iter()
            .filter(|item| !item.name.ends_with("Set"))
            .min_by_key(|item| levenshtein(&item.drop_name, needle));

        best_match.and_then(|item| {
            if levenshtein(&item.drop_name.replace(' ', ""), needle)
                <= threshold.unwrap_or(item.drop_name.len() / 3)
            {
                Some(item)
            } else {
                None
            }
        })
    }

    pub fn find_item_exact(&self, needle: &str) -> Option<&Item> {
        self.items.iter().find(|item| item.name == needle)
    }

    fn relic_to_bucket(&self, relic: &Relic, refinement: Refinement) -> Bucket {
        let common_chance = refinement.common_chance();
        let uncommon_chance = refinement.uncommon_chance();
        let rare_chance = refinement.rare_chance();

        let item_names = [
            (&relic.common1, common_chance),
            (&relic.common2, common_chance),
            (&relic.common3, common_chance),
            (&relic.uncommon1, uncommon_chance),
            (&relic.uncommon2, uncommon_chance),
            (&relic.rare1, rare_chance),
        ];
        let items = item_names
            .into_iter()
            .map(|(name, chance)| statistics::Item {
                value: match self.find_item_exact(name) {
                    Some(item) => item.platinum,
                    None => {
                        warn!("Failed to find item {name} in database");
                        0.0
                    }
                },
                probability: chance,
            })
            .collect();
        Bucket::new(items)
    }

    /// Relics of one era ("Lith", "Meso", "Neo" or "Axi", case insensitive)
    pub fn relics_of_era(&self, era: &str) -> Option<&HashMap<String, Relic>> {
        match era.to_lowercase().as_str() {
            "lith" => Some(&self.relics.lith),
            "meso" => Some(&self.relics.meso),
            "neo" => Some(&self.relics.neo),
            "axi" => Some(&self.relics.axi),
            _ => None,
        }
    }

    /// Rewards of a random intact relic of this era, preferring relics that still drop
    fn era_bucket(&self, era: &str) -> Bucket {
        let relics = self.relics_of_era(era).expect("Invalid relic era");
        let mut buckets: Vec<_> = relics
            .values()
            .filter(|relic| !relic.vaulted)
            .map(|relic| self.relic_to_bucket(relic, Refinement::Intact))
            .collect();
        if buckets.is_empty() {
            buckets = relics
                .values()
                .map(|relic| self.relic_to_bucket(relic, Refinement::Intact))
                .collect();
        }
        Bucket::mixture(&buckets)
    }

    /// Expected platinum of the reward picked in a public squad: the best of this relic's
    /// reward and the rewards of three other players opening random intact relics of the same era
    pub fn public_relic_value(&self, relic: &Relic, era: &str, refinement: Refinement) -> f32 {
        self.public_relic_value_against(relic, refinement, &self.era_bucket(era))
    }

    fn public_relic_value_against(
        &self,
        relic: &Relic,
        refinement: Refinement,
        others: &Bucket,
    ) -> f32 {
        let own = self.relic_to_bucket(relic, refinement);
        statistics::expectation_of_best(&[&own, others, others, others])
    }

    /// Value of a relic at each refinement and which refinement is worth its Void Traces.
    ///
    /// A refinement is only recommended when it gains more than `trace_threshold` platinum per trace.
    pub fn refinement_advice(&self, relic: &Relic, era: &str, trace_threshold: f32) -> RelicAdvice {
        let others = self.era_bucket(era);
        let values = Refinement::ALL.map(|refinement| RefinementValue {
            refinement,
            platinum: self.public_relic_value_against(relic, refinement, &others),
            platinum_per_trace: 0.0,
        });
        let intact = values[0].platinum;
        let values = values.map(|value| RefinementValue {
            platinum_per_trace: match value.refinement.trace_cost() {
                0 => 0.0,
                cost => (value.platinum - intact) / cost as f32,
            },
            ..value
        });
        RelicAdvice {
            values,
            trace_threshold,
        }
    }

    pub fn single_relic_value(&self, relic: &Relic, refinement: Refinement) -> f32 {
        let common_chance = refinement.common_chance();
        let uncommon_chance = refinement.uncommon_chance();
        let rare_chance = refinement.rare_chance();

        let value = 0.0
            + self.find_item_exact(&relic.common1).unwrap().platinum * common_chance
            + self.find_item_exact(&relic.common2).unwrap().platinum * common_chance
            + self.find_item_exact(&relic.common3).unwrap().platinum * common_chance
            + self.find_item_exact(&relic.uncommon1).unwrap().platinum * uncommon_chance
            + self.find_item_exact(&relic.uncommon2).unwrap().platinum * uncommon_chance
            + self.find_item_exact(&relic.rare1).unwrap().platinum * rare_chance;

        let item_names = [
            (&relic.common1, common_chance),
            (&relic.common2, common_chance),
            (&relic.common3, common_chance),
            (&relic.uncommon1, uncommon_chance),
            (&relic.uncommon2, uncommon_chance),
            (&relic.rare1, rare_chance),
        ];
        let value2: f32 = item_names
            .into_iter()
            .map(|(name, chance)| {
                let plat = self.find_item_exact(name).unwrap().platinum;
                println!("{plat} * {chance}");
                plat * chance
            })
            .sum();
        println!("{value} vs {value2}");

        value
    }

    pub fn shared_relic_value(
        &self,
        relic: &Relic,
        refinement: Refinement,
        number_of_relics: u32,
    ) -> f32 {
        let bucket = self.relic_to_bucket(relic, refinement);
        bucket.expectation_of_best_of_n(number_of_relics)
    }

    pub fn shared_relic_value_bruteforce(
        &self,
        relic: &Relic,
        refinement: Refinement,
        _number_of_relics: u32,
    ) -> f32 {
        let common_chance = refinement.common_chance();
        let uncommon_chance = refinement.uncommon_chance();
        let rare_chance = refinement.rare_chance();

        let items = [
            (&relic.common1, common_chance),
            (&relic.common2, common_chance),
            (&relic.common3, common_chance),
            (&relic.uncommon1, uncommon_chance),
            (&relic.uncommon2, uncommon_chance),
            (&relic.rare1, rare_chance),
        ];

        let mut value = 0.0;
        for item1 in items.iter() {
            for item2 in items.iter() {
                for item3 in items.iter() {
                    for item4 in items.iter() {
                        value += [item1.0, item2.0, item3.0, item4.0]
                            .iter()
                            .map(|name| self.find_item_exact(name).unwrap().platinum)
                            .max_by(|a, b| a.total_cmp(b))
                            .unwrap()
                            * item1.1
                            * item2.1
                            * item3.1
                            * item4.1
                    }
                }
            }
        }

        value
    }
}

fn remove_empty_relics_from_json(value: &mut Value) {
    let relics = &mut value["relics"];
    for (_, kind) in relics.as_object_mut().unwrap() {
        kind.as_object_mut()
            .unwrap()
            .retain(|_name, relic| serde_json::from_value::<Relic>(relic.clone()).is_ok());
    }
}

#[cfg(test)]
mod test {
    use approx::assert_relative_eq;

    use super::*;

    #[test]
    pub fn can_load_database() {
        Database::load_from_file(None, None);
    }

    #[test]
    pub fn can_find_items() {
        let db = Database::load_from_file(None, None);

        let item = db
            .find_item("TitaniaPrimeBlueprint", Some(0))
            .expect("Failed to find Titania Prime Blueprint in database");
        assert_eq!(item.name, "Titania Prime Blueprint");

        let item = db
            .find_item("OctaviaPrimeBlueprint", Some(0))
            .expect("Failed to find Octavia Prime Blueprint in database");
        assert_eq!(item.name, "Octavia Prime Blueprint");
    }

    #[test]
    pub fn can_find_fuzzy_items() {
        let db = Database::load_from_file(None, None);

        let item = db
            .find_item("Akstlett Prlme Recver", None)
            .expect("Failed to fuzzy find Akstiletto Prime Receiver in database");
        assert_eq!(item.name, "Akstiletto Prime Receiver");

        let item = db
            .find_item("ctavio Prlme Blueprnt", None)
            .expect("Failed to fuzzy find Octavia Prime Blueprint in database");
        assert_eq!(item.name, "Octavia Prime Blueprint");

        let item = db
            .find_item("Oclavia Prime Syslems\nBlueprint\n", None)
            .expect("Failed to fuzzy find Octavia Prime Blueprint in database");
        assert_eq!(item.name, "Octavia Prime Systems");
    }

    #[test]
    fn validate_shared_relic_values() {
        let database = Database::load_from_file(None, None);

        for (name, relic) in database.relics.lith.iter() {
            println!("{} {:#?}", name, relic);
            assert_relative_eq!(
                database.shared_relic_value(relic, Refinement::Radiant, 4),
                database.shared_relic_value_bruteforce(relic, Refinement::Radiant, 4),
                epsilon = 0.01
            )
        }
        for (name, relic) in database.relics.meso.iter() {
            println!("{} {:#?}", name, relic);
            assert_relative_eq!(
                database.shared_relic_value(relic, Refinement::Radiant, 4),
                database.shared_relic_value_bruteforce(relic, Refinement::Radiant, 4),
                epsilon = 0.01
            )
        }
        for (name, relic) in database.relics.neo.iter() {
            println!("{} {:#?}", name, relic);
            assert_relative_eq!(
                database.shared_relic_value(relic, Refinement::Radiant, 4),
                database.shared_relic_value_bruteforce(relic, Refinement::Radiant, 4),
                epsilon = 0.01
            )
        }
        for (name, relic) in database.relics.axi.iter() {
            println!("{} {:#?}", name, relic);
            assert_relative_eq!(
                database.shared_relic_value(relic, Refinement::Radiant, 4),
                database.shared_relic_value_bruteforce(relic, Refinement::Radiant, 4),
                epsilon = 0.01
            )
        }
    }
}
