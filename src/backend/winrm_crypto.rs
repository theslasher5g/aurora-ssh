//! MD4, MD5 and HMAC-MD5 – the three primitives NTLMv2 needs (NT hash, response keys).
//! Small, dependency-free implementations; they are only used for the NTLM handshake with
//! a Windows machine (these hashes are broken for any other purpose) and are checked
//! against the RFC 1320 / 1321 / 2202 test vectors below.

fn pad(data: &[u8]) -> Vec<u8> {
    let mut m = data.to_vec();
    m.push(0x80);
    while m.len() % 64 != 56 {
        m.push(0);
    }
    m.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_le_bytes());
    m
}

fn words(block: &[u8]) -> [u32; 16] {
    let mut x = [0u32; 16];
    for (i, w) in x.iter_mut().enumerate() {
        *w = u32::from_le_bytes([
            block[i * 4],
            block[i * 4 + 1],
            block[i * 4 + 2],
            block[i * 4 + 3],
        ]);
    }
    x
}

fn digest(state: [u32; 4]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (i, w) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

/// MD4 (RFC 1320).
pub fn md4(data: &[u8]) -> [u8; 16] {
    let mut st: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
    for block in pad(data).chunks_exact(64) {
        let x = words(block);
        let [mut a, mut b, mut c, mut d] = st;
        let f = |x: u32, y: u32, z: u32| (x & y) | (!x & z);
        let g = |x: u32, y: u32, z: u32| (x & y) | (x & z) | (y & z);
        let h = |x: u32, y: u32, z: u32| x ^ y ^ z;
        const R1: [u32; 4] = [3, 7, 11, 19];
        const R2: [u32; 4] = [3, 5, 9, 13];
        const R3: [u32; 4] = [3, 9, 11, 15];
        const K2: [usize; 16] = [0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15];
        const K3: [usize; 16] = [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15];
        for i in 0..16 {
            let t = a
                .wrapping_add(f(b, c, d))
                .wrapping_add(x[i])
                .rotate_left(R1[i % 4]);
            (a, d, c, b) = (d, c, b, t);
        }
        for i in 0..16 {
            let t = a
                .wrapping_add(g(b, c, d))
                .wrapping_add(x[K2[i]])
                .wrapping_add(0x5a827999)
                .rotate_left(R2[i % 4]);
            (a, d, c, b) = (d, c, b, t);
        }
        for i in 0..16 {
            let t = a
                .wrapping_add(h(b, c, d))
                .wrapping_add(x[K3[i]])
                .wrapping_add(0x6ed9eba1)
                .rotate_left(R3[i % 4]);
            (a, d, c, b) = (d, c, b, t);
        }
        st[0] = st[0].wrapping_add(a);
        st[1] = st[1].wrapping_add(b);
        st[2] = st[2].wrapping_add(c);
        st[3] = st[3].wrapping_add(d);
    }
    digest(st)
}

/// MD5 (RFC 1321).
pub fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64)
        .map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32)
        .collect();
    let mut st: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
    for block in pad(data).chunks_exact(64) {
        let m = words(block);
        let [mut a, mut b, mut c, mut d] = st;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        st[0] = st[0].wrapping_add(a);
        st[1] = st[1].wrapping_add(b);
        st[2] = st[2].wrapping_add(c);
        st[3] = st[3].wrapping_add(d);
    }
    digest(st)
}

/// HMAC-MD5 (RFC 2104).
pub fn hmac_md5(key: &[u8], msg: &[u8]) -> [u8; 16] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..16].copy_from_slice(&md5(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    inner.extend_from_slice(msg);
    let mut outer: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    outer.extend_from_slice(&md5(&inner));
    md5(&outer)
}

/// Lower-case hex (tests).
#[cfg(test)]
pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md4_vectors() {
        // RFC 1320 appendix A.5.
        assert_eq!(hex(&md4(b"")), "31d6cfe0d16ae931b73c59d7e0c089c0");
        assert_eq!(hex(&md4(b"a")), "bde52cb31de33e46245e05fbdbd6fb24");
        assert_eq!(hex(&md4(b"abc")), "a448017aaf21d8525fc10ae87aa6729d");
        assert_eq!(
            hex(&md4(b"message digest")),
            "d9130a8164549fe818874806e1c7014b"
        );
        assert_eq!(
            hex(&md4(
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
            )),
            "e33b4ddc9c38f2199c3e7b164fcc0536"
        );
    }

    #[test]
    fn md5_vectors() {
        // RFC 1321 appendix A.5.
        assert_eq!(hex(&md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(&md5(b"a")), "0cc175b9c0f1b6a831c399e269772661");
        assert_eq!(hex(&md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            hex(&md5(b"message digest")),
            "f96b697d7cb7938d525a2f31aaf161d0"
        );
        assert_eq!(
            hex(&md5(
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
            )),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
    }

    #[test]
    fn hmac_md5_vectors() {
        // RFC 2202 test cases 1, 2 and 6 (a key longer than the block size).
        assert_eq!(
            hex(&hmac_md5(&[0x0b; 16], b"Hi There")),
            "9294727a3638bb1c13f48ef8158bfc9d"
        );
        assert_eq!(
            hex(&hmac_md5(b"Jefe", b"what do ya want for nothing?")),
            "750c783e6ab0b503eaa86e310a5db738"
        );
        assert_eq!(
            hex(&hmac_md5(
                &[0xaa; 80],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "6b1ab7fe4bd7bf8f0b62e6ce61b9d0cd"
        );
    }
}
