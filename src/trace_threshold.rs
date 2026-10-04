use crate::{
    database::{Database, RelicAdvice},
    wfinfo_data::item_data::Refinement,
};

/// Void Traces earned by a fissure mission: 6 to 30, see https://wiki.warframe.com/w/Void_Traces
pub const AVERAGE_TRACES_PER_MISSION: f32 = 18.0;

/// What following the refinement advice costs and earns, on average over all relics
#[derive(Clone, Copy, Debug)]
pub struct Spending {
    pub threshold: f32,
    pub traces_per_relic: f32,
    pub platinum_per_relic: f32,
    /// Share of relics worth refining, from 0 to 1
    pub refined_share: f32,
}

/// Refinement values of every relic, computed once to try many thresholds
pub fn all_relic_advice(database: &Database) -> Vec<RelicAdvice> {
    ["Lith", "Meso", "Neo", "Axi"]
        .into_iter()
        .flat_map(|era| {
            database
                .relics_of_era(era)
                .unwrap()
                .values()
                .map(move |relic| database.refinement_advice(relic, era, 0.0))
        })
        .collect()
}

pub fn spending(advices: &[RelicAdvice], threshold: f32) -> Spending {
    let (mut traces, mut platinum, mut refined) = (0.0, 0.0, 0.0);
    for advice in advices {
        let advice = RelicAdvice {
            trace_threshold: threshold,
            ..*advice
        };
        if let Some((refinement, _)) = advice.recommendation(Refinement::Intact) {
            traces += refinement.trace_cost() as f32;
            platinum +=
                advice.value(refinement).platinum - advice.value(Refinement::Intact).platinum;
            refined += 1.0;
        }
    }
    let count = advices.len().max(1) as f32;
    Spending {
        threshold,
        traces_per_relic: traces / count,
        platinum_per_relic: platinum / count,
        refined_share: refined / count,
    }
}

/// Threshold at which refining spends traces as fast as they are earned, so that they neither
/// run out nor pile up to the cap
pub fn balanced_threshold(advices: &[RelicAdvice], traces_per_relic: f32) -> f32 {
    // Spending only goes down when the threshold goes up
    let (mut low, mut high) = (0.0_f32, 0.2_f32);
    for _ in 0..30 {
        let middle = (low + high) / 2.0;
        if spending(advices, middle).traces_per_relic > traces_per_relic {
            low = middle;
        } else {
            high = middle;
        }
    }
    high
}
