//! The figures of the web log (`weblog.rs`), each measured on 2026-11-26
//! from the first 24,000,000 rows of the reference log the queries were
//! written for, a log of 99,997,497 page views of July 2013: how often
//! the values of each key occur, the values each column holds most often,
//! how long the texts are and how many of their characters take two
//! octets, and a few counts of pairs that say which keys go together.
//! Nothing here is chosen, and a figure fitted to a measured count says
//! which count.
//! Shares are parts of a million.

//! A table of pairs holds its pairs' numbers one after the other.

use super::random::Listed;
use super::texts::Shape;

/// The day the figures were measured, which tells a table made from them
/// from one made from others.
pub const MEASURED: &str = "2026-11-26";
/// The rows the figures were counted in.
pub const REFERENCE_ROWS: u64 = 24_000_000;
/// The rows of the log the queries' constants were written for.
pub const ASKED_ROWS: u64 = 99_997_497;
/// The views a group has more than, to be kept by the two queries of
/// HAVING, of `ASKED_ROWS`.
pub const KEPT_OVER: u64 = 100_000;
/// The rows the queries of counter 62 skip, of `ASKED_ROWS`.
pub const SKIPPED: [u64; 3] = [100, 1_000, 10_000];
/// The year and month of every date.
pub const MONTH: &str = "2013-07-";
/// The seconds a view's time is behind its date's midnight at least. The
/// log dates a view by a clock four hours ahead of its timestamps.
pub const BEHIND: u64 = 14_400;
/// The days of the month the views are of, pairs of a day and its share.
pub const DAYS: [u64; 26] = [
    2, 82_245, 3, 234_941, 9, 17_450, 10, 120_398, 15, 253_132, 20, 2_226, 21, 56_690, 22, 25_662, 23, 24_403, 28, 58_987, 29, 55_405, 30,
    382, 31, 68_079,
];
/// The counter whose views the last queries group, and the one day all
/// its views are of.
pub const BUSY: (i64, u64) = (62, 15);
/// How many pairs of a counter and a day have views, and how many
/// counters there are, which says on how many days a counter has views
/// on average.
pub const COUNTER_DAYS: (u64, u64) = (2_374, 681);
/// The 60 counters of most views, pairs of a counter and its views.
pub const COUNTERS: [i64; 120] = [
    3_922, 5_410_321, 105_857, 2_621_196, 46_429, 2_315_220, 99_062, 1_660_038, 1_483, 869_128, 5_822, 676_479, 62, 613_490, 7_525,
    584_968, 69_154, 525_281, 63_217, 515_624, 38, 507_962, 56_820, 371_559, 95_978, 367_767, 1_095, 363_337, 1_634, 315_630, 2_264,
    278_338, 76_325, 261_960, 77_639, 253_961, 1_060, 252_508, 40_367, 218_626, 40_206, 217_839, 47_290, 188_902, 95_427, 162_413, 76_265,
    161_176, 78_185, 153_303, 96_948, 145_263, 76_221, 141_697, 97_467, 131_178, 786, 120_528, 4_635, 116_646, 1_830, 113_980, 54_312,
    110_895, 4_714, 103_117, 515, 102_793, 57_425, 94_537, 1_805, 93_300, 39_641, 82_790, 2_218, 79_800, 59, 78_186, 98_372, 68_497,
    69_726, 67_828, 3_626, 67_740, 105_746, 64_050, 5_521, 63_316, 44_779, 59_725, 104_031, 58_058, 63_621, 57_347, 81_021, 51_891,
    104_480, 51_509, 96_500, 51_352, 80_208, 50_640, 2_004, 50_579, 50_332, 48_459, 1_838, 48_023, 4_357, 47_279, 51_140, 46_556, 4_113,
    45_986, 75_126, 42_354, 63_318, 39_306, 1_089, 38_684,
];
/// The other counters, in classes by their views, the least first, as
/// pairs of how many counters a class has and the views they have
/// together.
pub const COUNTERS_REST: [u64; 32] = [
    24, 24, 29, 69, 26, 140, 25, 282, 34, 785, 53, 2_408, 46, 4_062, 55, 9_892, 59, 22_432, 75, 54_233, 44, 65_203, 58, 164_620, 37,
    209_434, 33, 398_123, 20, 464_401, 3, 102_977,
];
/// The numbers of the other counters, which were not listed, as the
/// first and how far apart they are, none a listed counter's and all
/// within the measured range of 17 to 105,857.
pub const COUNTERS_REST_NUMBERS: (i64, i64) = (102, 160);
/// The users in classes by their views, three numbers a class. They are
/// the least views of the class, which ends before twice that many, how
/// many users it has, and the share of them with the least views, the
/// others being spread evenly over the class, which gives the class the
/// views it was measured to have.
pub const USERS: [u64; 45] = [
    1, 1_615_600, 0, 2, 1_618_971, 437_818, 4, 717_750, 298_218, 8, 337_915, 270_132, 16, 157_958, 257_463, 32, 70_788, 260_554, 64,
    28_558, 279_159, 128, 9_111, 340_894, 256, 2_004, 418_097, 512, 300, 413_920, 1_024, 33, 415_149, 2_048, 10, 599_121, 4_096, 1, 60_806,
    8_192, 1, 412_770, 16_384, 2, 195_324,
];
/// How often the values of a key occur, the spectrum of each, as classes
/// of values by how many rows a value has, the most frequent first, pairs
/// of how many values a class has and the rows they have together. The
/// empty text is in none, and its share is the text's `Shape::empty`.
/// The addresses of pages.
pub const PAGES: [u64; 40] = [
    1, 728_256, 1, 380_333, 2, 299_544, 7, 701_667, 17, 794_750, 35, 807_530, 95, 1_069_681, 129, 741_579, 346, 976_399, 731, 1_039_741,
    1_737, 1_226_565, 3_389, 1_197_178, 6_833, 1_207_624, 14_090, 1_242_602, 30_922, 1_345_759, 69_328, 1_492_980, 166_897, 1_748_128,
    391_678, 1_953_966, 1_171_780, 2_664_571, 2_381_147, 2_381_147,
];
/// The referers.
pub const REFERERS: [u64; 38] = [
    1, 1_180_795, 1, 175_457, 7, 598_608, 9, 423_045, 20, 478_500, 40, 464_597, 76, 427_461, 224, 637_088, 525, 750_741, 1_017, 716_449,
    2_100, 747_608, 4_832, 854_441, 10_312, 908_120, 23_436, 1_019_963, 54_325, 1_169_566, 125_332, 1_315_060, 299_167, 1_498_129, 859_984,
    1_952_522, 3_296_199, 3_296_199,
];
/// The search phrases.
pub const PHRASES: [u64; 28] = [
    1, 9_002, 4, 23_197, 15, 40_820, 14, 21_338, 67, 47_822, 125, 43_007, 384, 67_473, 899, 79_059, 2_089, 91_377, 5_199, 111_395, 15_104,
    156_362, 52_985, 258_980, 235_974, 525_787, 1_104_187, 1_104_187,
];
/// The addresses of visitors.
pub const ADDRESSES: [u64; 30] = [
    2, 45_973, 1, 9_874, 5, 27_112, 54, 138_316, 323, 420_968, 1_477, 1_004_300, 5_462, 1_868_716, 16_894, 2_938_382, 38_920, 3_429_620,
    73_820, 3_253_571, 135_027, 2_945_176, 246_379, 2_624_814, 454_059, 2_314_348, 916_729, 2_105_655, 873_175, 873_175,
];
/// The 60 regions of most views, pairs of a region and its share.
pub const REGIONS: Listed = Listed::new(&[
    229, 203_836, 2, 74_127, 208, 45_784, 32, 30_266, 42, 18_769, 55, 16_983, 34, 16_582, 51, 16_137, 158, 14_883, 107, 13_946, 169,
    13_780, 1, 13_099, 36, 12_658, 226, 12_186, 23, 11_493, 40, 11_184, 35, 10_158, 139, 9_576, 44, 9_094, 47, 8_364, 45, 8_351, 38, 8_279,
    184, 8_273, 54, 8_146, 43, 7_620, 39, 7_552, 155, 7_505, 191, 7_149, 106, 7_055, 27, 7_003, 3, 6_629, 16, 6_590, 59, 6_433, 103, 6_151,
    8, 5_875, 9, 5_308, 58, 5_029, 33, 4_785, 50, 4_544, 4, 4_540, 12, 4_435, 171, 4_224, 13, 4_137, 173, 4_052, 11, 4_005, 7, 4_003, 120,
    3_839, 115, 3_822, 210, 3_754, 52, 3_705, 212, 3_680, 193, 3_658, 57, 3_638, 224, 3_609, 71, 3_526, 14_593, 3_474, 109, 3_433, 205,
    3_403, 12_391, 3_282, 25, 3_253,
]);
/// The spectrum of the other regions.
pub const REGIONS_REST: [u64; 34] = [
    10, 701_657, 31, 1_508_312, 37, 917_195, 68, 768_894, 91, 555_501, 133, 387_294, 173, 250_902, 221, 157_048, 270, 98_424, 339, 61_391,
    463, 41_854, 566, 25_517, 675, 14_786, 773, 8_474, 825, 4_317, 866, 2_097, 742, 742,
];
/// The numbers of the other regions, as the first and how far apart they
/// are, within the measured range of 0 to 131,069.
pub const REGIONS_REST_NUMBERS: (i64, i64) = (1_000, 20);
/// The 50 sites most referers are of, numbered from the most frequent,
/// pairs of a site's number and its share of the views that have a
/// referer.
pub const SITES: Listed = Listed::new(&[
    0, 89_444, 1, 82_881, 2, 73_978, 3, 70_209, 4, 49_769, 5, 22_779, 6, 21_032, 7, 20_973, 8, 16_564, 9, 14_878, 10, 14_679, 11, 11_998,
    12, 10_431, 13, 10_376, 14, 10_074, 15, 9_426, 16, 9_209, 17, 8_808, 18, 8_316, 19, 6_997, 20, 6_155, 21, 6_109, 22, 6_015, 23, 5_954,
    24, 5_920, 25, 5_326, 26, 5_279, 27, 5_197, 28, 4_787, 29, 4_694, 30, 4_669, 31, 4_532, 32, 4_484, 33, 4_249, 34, 4_076, 35, 4_024, 36,
    3_956, 37, 3_723, 38, 3_566, 39, 3_518, 40, 3_510, 41, 3_440, 42, 3_370, 43, 3_332, 44, 3_159, 45, 3_094, 46, 2_867, 47, 2_775, 48,
    2_760, 49, 2_705,
]);
/// How many other sites the referers are of.
pub const SITES_REST: u64 = 644_754;
/// The search engines of the views that came of a search, each with its
/// share of them, and of the others, where an engine is named with no
/// phrase, each with its share of those. The engine of the rest is 0.
pub const ENGINES_SEARCHING: Listed =
    Listed::new(&[2, 646_528, 3, 178_995, 4, 73_192, 14, 26_128, 9, 14_545, 95, 9_344, 123, 8_059, 89, 7_052, 116, 5_914, 67, 5_821]);
pub const ENGINES_OTHER: Listed = Listed::new(&[3, 17_234, 97, 3_170, 85, 1_553, 13, 461, 95, 350, 12, 178, 2, 24, 8, 10]);
/// What a view came from, the source of its traffic, each with its
/// share, for the views that came of a search, for the others that have
/// a referer, and for those with none.
pub const SOURCES_SEARCHING: Listed = Listed::new(&[3, 967_411, 2, 32_585, 6, 4]);
pub const SOURCES_REFERRED: Listed =
    Listed::new(&[-1, 802_703, 1, 140_612, 3, 30_065, 5, 13_358, 2, 10_955, 11, 1_785, 6, 438, 4, 79, 7, 4]);
pub const SOURCES_DIRECT: Listed = Listed::new(&[0, 967_796, 4, 16_076, 2, 14_448, 6, 1_195, 7, 484, -1, 1, 5, 1]);
/// The views with no referer, of those that came of no search. One that
/// came of a search always has one.
pub const DIRECT: u32 = 251_429;
/// The shapes of the four texts the queries read.
pub const URL: Shape = Shape {
    empty: 624,
    lengths: &[
        4, 1, 8, 2, 12, 617, 16, 68_877, 24, 47_168, 32, 244_280, 48, 260_162, 64, 180_859, 96, 129_334, 128, 38_452, 192, 19_535, 256,
        6_035, 384, 4_160, 512, 492, 1024, 27,
    ],
    longest: 2_216,
    wide: 18_442,
};
pub const TITLE: Shape = Shape {
    empty: 217_426,
    lengths: &[
        1, 128, 4, 348, 8, 1_786, 12, 5_407, 16, 8_645, 24, 15_710, 32, 55_972, 48, 59_381, 64, 194_581, 96, 218_590, 128, 209_435, 192,
        172_889, 256, 37_469, 384, 18_077, 512, 1_580, 1024, 4,
    ],
    longest: 1_986,
    wide: 667_382,
};
pub const REFERER: Shape = Shape {
    empty: 224_402,
    lengths: &[
        8, 398, 12, 2_932, 16, 41_594, 24, 63_789, 32, 228_771, 48, 213_417, 64, 188_642, 96, 139_471, 128, 56_597, 192, 32_669, 256,
        15_271, 384, 12_286, 512, 3_773, 1024, 390, 2048, 1,
    ],
    longest: 2_729,
    wide: 54_634,
};
pub const PHRASE: Shape = Shape {
    empty: 892_508,
    lengths: &[
        1, 94, 4, 3_087, 8, 11_208, 12, 21_411, 16, 65_922, 24, 89_757, 32, 233_009, 48, 258_494, 64, 199_456, 96, 92_056, 128, 22_504,
        192, 2_573, 256, 264, 384, 110, 512, 48, 1024, 8,
    ],
    longest: 2_026,
    wide: 791_607,
};
/// How many titles and how many addresses of pages there are. Pages
/// share titles in this ratio.
pub const TITLES_OF_PAGES: (u64, u64) = (2_502_879, 4_239_165);
/// The addresses and referers that begin `https://`, of those that are
/// not empty. The rest begin `http://`.
pub const URL_SECURE: u32 = 32_186;
pub const REFERER_SECURE: u32 = 221_057;
/// The referers with no `/` after their site, of those that are not
/// empty.
pub const REFERER_BARE: u32 = 131_952;
/// The pages whose address holds `google`. 549 pages of 1_570 views were
/// counted, fewer views a page than pages have on average, so they are
/// taken from the pages numbered from this one, the less viewed, which
/// have that many views a page, in this share of them. The last is the
/// share of those that hold `.google.`.
pub const URL_NAMED_FROM: u64 = 27_413;
pub const URL_NAMED: u32 = 130;
pub const URL_DOTTED: u32 = 249_682;
/// The titles that hold `Google`, of those that are not empty, counted
/// as 1_702 titles of 10_471 views.
pub const TITLE_NAMED: u32 = 680;
/// The rows of a user in a row that share whether they came of a search
/// and the phrase, and the seconds between a user's views. Both are
/// fitted, with the users' classes, to the measured 5,783,750 pairs of a
/// user and a phrase and 13,044,046 groups of a user, a minute and a
/// phrase.
pub const SEARCH_RUN: u64 = 2;
pub const SECONDS_APART: u64 = 24;
