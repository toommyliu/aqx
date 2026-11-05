const RANK_THRESHOLDS: [u32; 9] = [
    900, 3600, 10000, 22500, 44100, 78400, 129600, 202500, 302500,
];

pub(crate) fn rank(points: u32) -> u32 {
    1 + RANK_THRESHOLDS
        .iter()
        .filter(|&&floor| points >= floor)
        .count() as u32
}
