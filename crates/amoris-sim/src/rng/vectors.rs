//! The reference vectors of docs/spec/rng.md 4, 5.1 and 5.2 (`rng.vectors`), checked natively and
//! in the WebAssembly build.

use super::{KeyPart, Pcg32, derive, fnv1a64, key_words, mix64, stream_from_seed};
use crate::entity::EntityId;
use crate::time::Tick;

fn expect<T: PartialEq + std::fmt::Debug>(what: &str, got: T, want: T) -> Result<(), String> {
    if got == want {
        Ok(())
    } else {
        Err(format!("{what}: got {got:?}, want {want:?}"))
    }
}

/// Every vector; the first mismatch is the error.
pub fn check() -> Result<(), String> {
    // PCG32 seed 42, sequence 54: the reference pcg32-demo.
    let mut r = Pcg32::new(42, 54);
    let six: Vec<u32> = (0..6).map(|_| r.next_u32()).collect();
    expect(
        "pcg32(42, 54)",
        six,
        vec![
            0xa15c_02b7,
            0x7b47_f409,
            0xba1d_3330,
            0x83d2_f293,
            0xbfa4_784b,
            0xcbed_606e,
        ],
    )?;
    let coins: String = (0..65)
        .map(|_| match r.below(2) {
            Ok(1) => 'H',
            _ => 'T',
        })
        .collect();
    expect(
        "coins",
        coins.as_str(),
        "HHTTTHTHHHTHTTTHHHHHTTTHHHTHTHTHTTHTTTHHHHHHTTTTHHTTTTTHTTTTTTTHT",
    )?;
    let rolls: Vec<u32> = (0..33).map(|_| r.below(6).map_or(0, |x| x + 1)).collect();
    expect(
        "rolls",
        rolls,
        vec![
            3, 4, 1, 1, 2, 2, 3, 2, 4, 3, 2, 4, 3, 3, 5, 2, 3, 1, 3, 1, 5, 1, 4, 1, 5, 6, 4, 6, 6,
            2, 6, 3, 3,
        ],
    )?;

    // SplitMix64 seeded with 1234567.
    let mut s: u64 = 1_234_567;
    let mut next = || {
        s = s.wrapping_add(0x9e37_79b9_7f4a_7c15);
        mix64(s)
    };
    let sm: Vec<u64> = (0..5).map(|_| next()).collect();
    expect(
        "splitmix64(1234567)",
        sm,
        vec![
            6_457_827_717_110_365_317,
            3_203_168_211_198_807_973,
            9_817_491_932_198_370_423,
            4_593_380_528_125_082_431,
            16_408_922_859_458_223_821,
        ],
    )?;

    // FNV-1a 64 reference values.
    expect("fnv1a64('')", fnv1a64(b""), 0xcbf2_9ce4_8422_2325)?;
    expect("fnv1a64('a')", fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c)?;
    expect(
        "fnv1a64('foobar')",
        fnv1a64(b"foobar"),
        0x8594_4171_f739_67e8,
    )?;
    expect(
        "fnv1a64('script:spawner')",
        fnv1a64(b"script:spawner"),
        0x1aac_efc9_3a5c_ee94,
    )?;
    expect("derive(0, [])", derive(0, &[]), 0xe220_a839_7b1d_cdaf)?;

    // The stream encodings of rng.md 5.2, world seed 42.
    let entity = EntityId::new(17).ok_or("entity 17")?;
    let system = [KeyPart::Tick(Tick(600)), KeyPart::System("script:spawner")];
    let cases: [(&str, Vec<KeyPart<'_>>, u64, [u32; 4]); 5] = [
        (
            "system",
            system.to_vec(),
            0x0a78_4985_9791_245c,
            [0xdd62_dfd3, 0x9646_2238, 0xd45b_1aac, 0xe19d_0054],
        ),
        (
            "entity",
            [&system[..], &[KeyPart::Entity(entity)]].concat(),
            0x1328_745c_48e6_aef9,
            [0xa8e4_03aa, 0x6f5d_f988, 0x53c0_7740, 0x360b_b58b],
        ),
        (
            "named",
            [&system[..], &[KeyPart::Str("loot"), KeyPart::Int(3)]].concat(),
            0x739e_c45a_b668_7cd7,
            [0xa982_b9d2, 0xf306_d7a7, 0xc212_bf00, 0xb427_6979],
        ),
        (
            "timeless",
            vec![KeyPart::Str("level"), KeyPart::Int(7)],
            0x96ce_2cf4_9871_9dd7,
            [0x1ab7_557b, 0xd595_f07c, 0x4749_8bec, 0x0457_fad1],
        ),
        (
            "integer -1",
            vec![
                KeyPart::Tick(Tick(1)),
                KeyPart::System("s"),
                KeyPart::Int(-1),
            ],
            0xb22f_16b3_8242_e54c,
            [0xbbb7_0eed, 0xcff5_c38f, 0xea49_c07a, 0xb3be_5ab3],
        ),
    ];
    for (name, parts, seed, first) in cases {
        let words = key_words(&parts).map_err(|p| p.message)?;
        let d = derive(42, &words);
        expect(&format!("{name} derived seed"), d, seed)?;
        let mut g = stream_from_seed(d);
        let got = [g.next_u32(), g.next_u32(), g.next_u32(), g.next_u32()];
        expect(&format!("{name} first draws"), got, first)?;
    }
    Ok(())
}
