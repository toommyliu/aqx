const RANK_THRESHOLDS: [u32; 9] = [
    900, 3600, 10000, 22500, 44100, 78400, 129600, 202500, 302500,
];

pub(crate) fn rank(points: u32) -> u32 {
    1 + RANK_THRESHOLDS
        .iter()
        .filter(|&&floor| points >= floor)
        .count() as u32
}

pub(crate) fn progress(points: u32) -> (u32, Option<u32>) {
    let reached = RANK_THRESHOLDS
        .iter()
        .filter(|&&floor| points >= floor)
        .count();
    let floor = reached.checked_sub(1).map_or(0, |at| RANK_THRESHOLDS[at]);
    let next = RANK_THRESHOLDS.get(reached).map(|next| next - floor);
    (points - floor, next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_counts_from_the_current_rank() {
        assert_eq!((rank(0), progress(0)), (1, (0, Some(900))));
        assert_eq!((rank(1000), progress(1000)), (2, (100, Some(2700))));
        assert_eq!((rank(3600), progress(3600)), (3, (0, Some(6400))));
        assert_eq!((rank(302_500), progress(302_600)), (10, (100, None)));
    }
}
