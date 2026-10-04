#[derive(Copy, Clone, Debug)]
pub struct Item {
    pub value: f32,
    pub probability: f32,
}

#[derive(Clone, Debug)]
pub struct Bucket {
    items: Vec<Item>,
}

impl Bucket {
    pub fn new(mut items: Vec<Item>) -> Self {
        items.sort_by(|a, b| a.value.total_cmp(&b.value));
        Self { items }
    }

    fn cumulative(&self) -> Vec<Item> {
        let mut probability_sum = 0.0;
        self.items
            .iter()
            .map(|item| {
                probability_sum += item.probability;
                Item {
                    value: item.value,
                    probability: probability_sum,
                }
            })
            .collect()
    }

    pub fn expectation_of_best_of_n(&self, n: u32) -> f32 {
        let cdf = self.cumulative();

        let mut total_expectation = 0.0;
        let mut previous_probability = 0.0;
        for item in cdf.iter() {
            let cumulative_probability = item.probability.powi(n as i32);
            let just_this_probability = cumulative_probability - previous_probability;
            previous_probability = cumulative_probability;
            total_expectation += just_this_probability * item.value;
        }

        total_expectation
    }

    /// Probability that a draw from this bucket is worth at most `value`
    fn cdf(&self, value: f32) -> f32 {
        self.items
            .iter()
            .filter(|item| item.value <= value)
            .map(|item| item.probability)
            .sum()
    }

    /// Uniform mixture of several buckets, e.g. "a random relic of this era"
    pub fn mixture(buckets: &[Bucket]) -> Self {
        let weight = 1.0 / buckets.len() as f32;
        Self::new(
            buckets
                .iter()
                .flat_map(|bucket| bucket.items.iter())
                .map(|item| Item {
                    value: item.value,
                    probability: item.probability * weight,
                })
                .collect(),
        )
    }
}

/// Expected value of the best of one independent draw from each bucket
pub fn expectation_of_best(buckets: &[&Bucket]) -> f32 {
    let mut values: Vec<f32> = buckets
        .iter()
        .flat_map(|bucket| bucket.items.iter().map(|item| item.value))
        .collect();
    values.sort_by(|a, b| a.total_cmp(b));
    values.dedup();

    let mut total_expectation = 0.0;
    let mut previous_probability = 0.0;
    for value in values {
        let cumulative_probability: f32 = buckets.iter().map(|bucket| bucket.cdf(value)).product();
        total_expectation += (cumulative_probability - previous_probability) * value;
        previous_probability = cumulative_probability;
    }

    total_expectation
}

#[cfg(test)]
mod test {
    use approx::assert_relative_eq;

    use super::*;

    fn uniform(values: Vec<f32>) -> Vec<Item> {
        let probability = 1.0 / values.len() as f32;
        values
            .into_iter()
            .map(|value| Item { value, probability })
            .collect()
    }

    #[test]
    fn single_die() {
        let bucket = Bucket::new(uniform(vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]));
        let value = bucket.expectation_of_best_of_n(1);
        assert_relative_eq!(value, 3.5);
    }

    #[test]
    fn two_dies() {
        let bucket = Bucket::new(uniform(vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]));
        let value = bucket.expectation_of_best_of_n(2);
        assert_relative_eq!(value, 161.0 / 36.0);
    }

    #[test]
    fn best_of_identical_buckets_matches_best_of_n() {
        let bucket = Bucket::new(uniform(vec![1.0, 2.0, 2.0, 5.0, 8.0, 13.0]));
        assert_relative_eq!(
            expectation_of_best(&[&bucket, &bucket, &bucket, &bucket]),
            bucket.expectation_of_best_of_n(4),
            epsilon = 1e-4
        );
    }

    #[test]
    fn best_of_different_buckets() {
        let die = Bucket::new(uniform(vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]));
        let coin = Bucket::new(uniform(vec![0.0, 10.0]));
        // Half the time the coin gives 10, otherwise the die decides
        assert_relative_eq!(expectation_of_best(&[&die, &coin]), 0.5 * 10.0 + 0.5 * 3.5);
    }

    #[test]
    fn odd_coin() {
        let bucket = Bucket::new(vec![
            Item {
                value: 0.0,
                probability: 0.9,
            },
            Item {
                value: 1.0,
                probability: 0.1,
            },
        ]);
        let value = bucket.expectation_of_best_of_n(1);
        assert_relative_eq!(value, 0.1);
    }

    #[test]
    fn odd_coins() {
        let bucket = Bucket::new(vec![
            Item {
                value: 0.0,
                probability: 0.9,
            },
            Item {
                value: 1.0,
                probability: 0.1,
            },
        ]);
        let value = bucket.expectation_of_best_of_n(2);
        assert_relative_eq!(value, 0.1 + 0.9 * 0.1);
    }
}
