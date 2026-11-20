//! zlib's tests: RFC vectors, streams zlib itself wrote, round trips,
//! the inflate bound and malformed streams.

use super::*;

fn hex(text: &str) -> Vec<u8> {
    text.as_bytes().chunks(2).map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap()).collect()
}

/// 300 letters of a fixed LCG, skewed to `e`, `t` and `a`: text a
/// dynamic Huffman block suits.
fn skewed() -> Vec<u8> {
    let letters = b"eeeeeeeetttttaaaaooooiinnsshhrdlu ";
    let mut x: u64 = 12345;
    (0..300)
        .map(|_| {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345) & 0x7fff_ffff;
            letters[usize::try_from((x >> 16_u32) % 34).unwrap()]
        })
        .collect()
}

/// Bytes of a fixed xorshift, which do not compress.
fn noise(n: usize) -> Vec<u8> {
    let mut x: u32 = 0x9E37_79B9;
    (0..n)
        .map(|_| {
            x ^= x << 13_u32;
            x ^= x >> 17_u32;
            x ^= x << 5_u32;
            x.to_le_bytes()[0]
        })
        .collect()
}

/// Verifies: [EXTENSION] RFC 1950 §8.2, §9 (RULES.md D193): Adler-32 is
/// 1 of no bytes, and matches zlib's of "Wikipedia" and of long inputs
/// that pass NMAX before each reduction.
#[test]
fn adler32_matches_rfc_1950() {
    assert_eq!(adler32(b""), 1);
    assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    assert_eq!(adler32(&vec![0xFF; 100_000]), 0x149A_302C);
    let repeated: Vec<u8> = (0..20_u8).flat_map(|_| 0..=255_u8).collect();
    assert_eq!(adler32(&repeated), 0x23C4_F688);
    let (a, b) = repeated.split_at(1000);
    assert_eq!(adler::update(adler32(a), b), adler32(&repeated));
}

/// Verifies: [EXTENSION] RFC 1950 §2.2, RFC 1951 §3.2.4, §3.2.6, §3.2.7
/// (RULES.md D193): streams zlib 1.2.13 wrote, of a stored block, fixed
/// Huffman blocks and a dynamic Huffman block, inflate to their data.
#[test]
fn inflates_streams_zlib_wrote() {
    let limit = 1_usize << 20_u32;
    assert_eq!(decompress(&hex("789c030000000001"), limit).unwrap(), b"");
    assert_eq!(decompress(&hex("789c4b040000620062"), limit).unwrap(), b"a");
    assert_eq!(decompress(&hex("789ccb48cdc9c957c8402701680308b1"), limit).unwrap(), b"hello hello hello hello");
    assert_eq!(decompress(&hex("7801010f00f0ff747572626f72656c2073746f72656431df0621"), limit).unwrap(), b"turborel stored");
    let dynamic = hex(concat!(
        "78da1d8f510a04310c42afd2ab092324502a4cecfdd7d9f6cf18f3c4e87977cbdd609e80da30e9439167cae2921efbfa4e347c26dd81d455a6bc1ccdda",
        "c667dea06d81739277e9184fa2142953aafa54c594df245ab875a585f9a41c46b589e1e0c1f924aed5ee69d7127b88ed07f848bab6fed79b5e1af835",
        "976767256197152ee2453cbaf13e507a9eb49c10255c4a817314e40a4db6b3997e0c7cf986dbe8d065be7ff66d7a07"
    ));
    assert_eq!((dynamic[2] >> 1_u32) & 3, 2, "the vector's first block is dynamic");
    assert_eq!(decompress(&dynamic, limit).unwrap(), skewed());
}

/// Verifies: [EXTENSION] RFC 1950 §2.2, RFC 1951 §3.2 (RULES.md D193):
/// what `compress` writes, `decompress` reads back, for no bytes, one,
/// every byte value, text, long runs past the window and the longest
/// match, and noise; text and runs come out smaller, and noise no more
/// than stored blocks' overhead larger.
#[test]
fn compress_round_trips() {
    let text = b"SELECT NAME, SALARY FROM STAFF WHERE SALARY > 50000; ".repeat(40);
    let runs: Vec<u8> = (0..200_000_u32).map(|i| u8::try_from(i.checked_div(1000).unwrap() % 7).unwrap()).collect();
    let all: Vec<u8> = (0..=255_u8).collect();
    let cases: [&[u8]; 7] = [b"", b"x", &all, &text, &runs, &noise(100_000), &skewed()];
    for data in cases {
        let stream = compress(data);
        assert_eq!(decompress(&stream, data.len()).unwrap(), data, "{} bytes", data.len());
        assert_eq!(&stream[..2], &[0x78, 0x9C]);
    }
    assert!(compress(&text).len().saturating_mul(10) < text.len());
    assert!(compress(&runs).len().saturating_mul(50) < runs.len());
    let noisy = noise(100_000);
    assert!(compress(&noisy).len() <= noisy.len() + 6 + 5 * 4, "noise is sent in stored blocks");
}

/// Verifies: [EXTENSION] RFC 1951 §3.2.7 (RULES.md D193): a block that
/// compresses is sent with dynamic Huffman codes, and the skewed text's
/// stream is no longer than zlib's own at level 9 plus a tenth.
#[test]
fn compress_writes_dynamic_blocks() {
    let stream = compress(&skewed());
    assert_eq!((stream[2] >> 1_u32) & 3, 2);
    assert!(stream.len() <= 168 + 17, "{} bytes", stream.len());
}

/// Verifies: [EXTENSION] RULES.md D193: inflation stops at its limit, so
/// a megabyte of zeros, about a kilobyte compressed, is refused under a
/// smaller limit as too large, and read under its own length.
#[test]
fn inflate_is_bounded() {
    let zeros = vec![0_u8; 1 << 20];
    let bomb = compress(&zeros);
    assert!(bomb.len() < 2048);
    let e = decompress(&bomb, 4096).unwrap_err();
    assert!(is_too_large(&e), "{e}");
    assert_eq!(decompress(&bomb, zeros.len()).unwrap().len(), zeros.len());
    assert!(is_too_large(&decompress(&bomb, zeros.len() - 1).unwrap_err()));
    let stored = hex("7801010f00f0ff747572626f72656c2073746f72656431df0621");
    assert!(is_too_large(&decompress(&stored, 14).unwrap_err()));
}

/// Verifies: [EXTENSION] RFC 1950 §2.2, RFC 1951 §3.2.3, §3.2.4, §3.2.5
/// (RULES.md D193): a stream is refused, never too large, for a method
/// other than deflate, a window past 32K, a failed header check, a
/// preset dictionary, a wrong or missing Adler-32, bytes after it, a cut
/// body, the reserved block type, a stored length unlike its complement,
/// and a distance before the data's start.
#[test]
fn malformed_streams_are_refused() {
    let good = hex("789c4b040000620062");
    let mut wrong_adler = good.clone();
    wrong_adler[8] ^= 1;
    let mut trailing = good.clone();
    trailing.push(0);
    let cases: [Vec<u8>; 11] = [
        hex("79"),
        hex("799c4b040000620062"),
        hex("889c4b040000620062"),
        hex("789d4b040000620062"),
        hex("78bb4b040000620062"),
        wrong_adler,
        trailing,
        good[..5].to_vec(),
        hex("789c07000000000001"),
        hex("7801010f00f1ff747572626f72656c2073746f72656431df0621"),
        // A fixed block: the literal 'a', then a match of length 3 at
        // distance 2, one byte before the start.
        hex("789c4b04420000000000"),
    ];
    for (i, stream) in cases.iter().enumerate() {
        let e = decompress(stream, 1 << 20).unwrap_err();
        assert!(!is_too_large(&e), "case {i}");
        assert_eq!(e.sqlcode(), -916_i32, "case {i}: {e}");
    }
}

/// Verifies: [EXTENSION] RFC 1951 §3.2.2 (RULES.md D193): a Huffman code
/// with more codes than its lengths allow is refused, a complete one and
/// one of a single code of one bit are taken, and the code length code
/// must be complete.
#[test]
fn huffman_codes_are_checked() {
    use super::inflate::huffman_check;
    assert!(huffman_check(&[1, 1, 1], false).is_err());
    assert!(huffman_check(&[1, 2, 2], false).is_ok());
    assert!(huffman_check(&[0, 1, 0], false).is_ok());
    assert!(huffman_check(&[0, 0, 0], false).is_ok());
    assert!(huffman_check(&[2, 2, 2], false).is_err());
    assert!(huffman_check(&[0, 1, 0], true).is_err());
    assert!(huffman_check(&[1, 1], true).is_ok());
}

/// Verifies: [EXTENSION] RFC 1951 §3.2.5, §3.2.6, §3.2.7 (RULES.md D193):
/// streams zlib 1.2.13 wrote of a match 32,100 bytes back, near the
/// window's end, at level 9, at level 1, of fixed Huffman codes alone and
/// of run lengths alone, inflate to their data.
#[test]
fn inflates_far_matches_zlib_wrote() {
    let a = noise(100);
    let data = [a.as_slice(), &vec![0; 32_000], &a].concat();
    let streams = [
        concat!(
            "78daedddb14a42010000c01c1a02c159149ac5688d228dc0a5b90f700a8cda128540274968686a0841840817411a14df12ba390415b408",
            "a165e01225b40511b4f40d4d773f72d1f4467779eda1bf1ecc3b8f9b41f67452daea4f4697f19db7b369be593bd81e1e97cbede7fa51a2",
            "53d84b359ff2773fa54c24579c2582f38bd5c1eefdebfb756be5e6b6371c549385d062e3fba57275328a1d7eeec797225fb1f0c7780100",
            "0000000000000000000000000000000000000000000000000000000000e04ff41f8ef55f11536b89",
        ),
        concat!(
            "7801eddd314a82010080d11c1a02c159149ac5688d228dc0a5b9033805466d8942a09324343435842042848b200d8aff12ba390415b408",
            "a165e01225b40511b47486a6f71de15de08ba637bacb6b0ffdf560de79dc0cb2a793d2567f32ba8cefbc9d4df3cddac1f6f0b85c6e3fd7",
            "8f129dc25eaaf994bffb296522b9e22c119c5fac0e76ef5fdfaf5b2b37b7bde1a09a2c84161bdf2f95ab9351ecf0733fbe14f98a853fc6",
            "0b224080000102040810204080000102040810204080000102040810204080000102040810204080000102040810204080000102040810",
            "20408000010204081020408000010204081020408000010204081020408000010204081020408000010204081020408000010204081020",
            "4080000102040810204080000102040810204080000102040810204080c09f40f41f1eebbf11536b89",
        ),
        concat!(
            "780193b4b3da2a6f7e61bfe58e775b2edbec88efba576bbfffde8d45329eaffa1e962c9b95ed78bcaaae6eddfdb9c59a5bca526d97dd2e",
            "39f7b7d6853fa3fca9e68e2933f40e049f7ff466c36a9dbda7b71f3fd0ae5dc6c83afff78396256d37a4733e66ca70f2ff90e6797b9761",
            "148c8251300a46c1281805a360148c8251300a46c1281805a360148c8251300a46c1281805a360148c8251300a46c1281805a360148c82",
            "51300a46c1281805a360148c8251300a46c1281805a360148c8251300a46c1281805a360148c8251300a46c1281805a360148c8251300a",
            "46c1281805a360148c8251300a46c1281805a360148c8251300a46c1281805a360148c8251300a46c1281805a360148c8251300a46c128",
            "1805a360148c8251300a46c1281805a360148c8251300a46c1281805a360148c8251300aa040920e77ac030011536b89",
        ),
        concat!(
            "7801edc13fcb010100c0e1d76050ea669d325f6415f993b2987d0093223617a5ee2651867732484949b728194e6ed1d96e5028cb95fc2d",
            "8b503625f52eefc7f83d8f271e99fa421b336c3cf56dd4c8b60e4ac23cd8036ffaf67b96b56e3169d554757cec9525bd928b693b79f555",
            "5242be7a958c7627b8c8ac2ff7c928305fceac45d35f7138fb9f537dd8b0c5d2abe075096fd1fdd8ff0000000000000000000000000000",
            "0000000000000000000000000000000000000000000000000000000000000000c03f4f3c32f5853666d878eadba8916d1d948479b007de",
            "f4edf72c6bdd62d2aaa9eaf8d82b4b7a2517d376f2eaaba4847cf52a19ed4e7091595fee935160be9c598ba6bfe270f63fa7fab0618ba5",
            "57c1eb12dea2fbb1ff0311536b89",
        ),
    ];
    for stream in streams {
        assert_eq!(decompress(&hex(stream), data.len()).unwrap(), data);
    }
}

/// Verifies: [EXTENSION] RFC 1950 §2.2, RFC 1951 §3.2 (RULES.md D193):
/// a stream cut short anywhere, in its header, inside a stored, fixed or
/// dynamic block, inside a dynamic block's code lengths, or in its
/// Adler-32, is refused as malformed (SQLCODE -916), never inflated to
/// less.
#[test]
fn every_prefix_of_a_stream_is_refused() {
    let fixed = hex("789c4b040000620062");
    let stored = compress(&noise(40));
    let dynamic = compress(&skewed());
    for stream in [fixed, stored, dynamic] {
        assert!(decompress(&stream, 1 << 20).is_ok());
        for end in 0..stream.len() {
            let e = decompress(&stream[..end], 1 << 20).unwrap_err();
            assert_eq!(e.sqlcode(), -916_i32, "cut at {end} of {}: {e}", stream.len());
        }
    }
}
