//! NTLMv2 authentication (MS-NLMP) for WinRM over HTTP(S): the three messages
//! (negotiate, challenge, authenticate) as bytes, without signing or sealing – the
//! transport is TLS, or the server explicitly allows unencrypted traffic. Pure functions;
//! the primitives are in `winrm_crypto`. Checked against the MS-NLMP section 4.2.4
//! test vectors.

use anyhow::{Result, anyhow, bail};

use super::winrm_crypto::{hmac_md5, md4};

const SIGNATURE: &[u8; 8] = b"NTLMSSP\0";

pub const NEGOTIATE_UNICODE: u32 = 0x0000_0001;
pub const REQUEST_TARGET: u32 = 0x0000_0004;
pub const NEGOTIATE_NTLM: u32 = 0x0000_0200;
pub const NEGOTIATE_ALWAYS_SIGN: u32 = 0x0000_8000;
pub const NEGOTIATE_EXTENDED_SESSIONSECURITY: u32 = 0x0008_0000;
pub const NEGOTIATE_TARGET_INFO: u32 = 0x0080_0000;
pub const NEGOTIATE_VERSION: u32 = 0x0200_0000;
pub const NEGOTIATE_128: u32 = 0x2000_0000;
pub const NEGOTIATE_56: u32 = 0x8000_0000;

/// What the client offers: Unicode, NTLMv2 (extended session security), target info.
pub const OFFERED: u32 = NEGOTIATE_UNICODE
    | REQUEST_TARGET
    | NEGOTIATE_NTLM
    | NEGOTIATE_ALWAYS_SIGN
    | NEGOTIATE_EXTENDED_SESSIONSECURITY
    | NEGOTIATE_TARGET_INFO
    | NEGOTIATE_VERSION
    | NEGOTIATE_128
    | NEGOTIATE_56;

/// Windows 6.1 build 7601, NTLM revision 15 (what clients commonly announce).
const VERSION: [u8; 8] = [6, 1, 0xb1, 0x1d, 0, 0, 0, 15];

const AV_EOL: u16 = 0;
const AV_FLAGS: u16 = 6;
const AV_TIMESTAMP: u16 = 7;

fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// Message 1.
pub fn negotiate_message() -> Vec<u8> {
    let mut m = Vec::with_capacity(40);
    m.extend_from_slice(SIGNATURE);
    m.extend_from_slice(&1u32.to_le_bytes());
    m.extend_from_slice(&OFFERED.to_le_bytes());
    // Domain and workstation: empty, pointing at the end of the header.
    for _ in 0..2 {
        m.extend_from_slice(&[0, 0, 0, 0]);
        m.extend_from_slice(&40u32.to_le_bytes());
    }
    m.extend_from_slice(&VERSION);
    m
}

/// What the server's challenge (message 2) carries.
#[derive(Debug, Clone, PartialEq)]
pub struct Challenge {
    pub flags: u32,
    pub server_challenge: [u8; 8],
    pub target_info: Vec<u8>,
    /// The message as received (the MIC covers it).
    pub raw: Vec<u8>,
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// The bytes a security buffer (length, max length, offset) points at.
fn buffer(b: &[u8], field: usize) -> Option<&[u8]> {
    let len = u16_at(b, field)? as usize;
    let off = u32_at(b, field + 4)? as usize;
    b.get(off..off.checked_add(len)?)
}

pub fn parse_challenge(msg: &[u8]) -> Result<Challenge> {
    if msg.len() < 32 || &msg[..8] != SIGNATURE {
        bail!("{}", t!("Keine gültige NTLM-Antwort des Servers"));
    }
    if u32_at(msg, 8) != Some(2) {
        bail!("{}", t!("Unerwarteter NTLM-Nachrichtentyp"));
    }
    let flags = u32_at(msg, 20).ok_or_else(|| anyhow!("NTLM challenge too short"))?;
    let mut server_challenge = [0u8; 8];
    server_challenge.copy_from_slice(&msg[24..32]);
    // Target info exists only if the message is long enough to have the field.
    let target_info = if msg.len() >= 48 {
        buffer(msg, 40).map(<[u8]>::to_vec).unwrap_or_default()
    } else {
        Vec::new()
    };
    Ok(Challenge {
        flags,
        server_challenge,
        target_info,
        raw: msg.to_vec(),
    })
}

/// `(type, value)` pairs of a target info block, up to the terminator.
pub fn av_pairs(info: &[u8]) -> Vec<(u16, Vec<u8>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let (Some(t), Some(l)) = (u16_at(info, i), u16_at(info, i + 2)) {
        let l = l as usize;
        if t == AV_EOL {
            break;
        }
        let Some(v) = info.get(i + 4..i + 4 + l) else {
            break;
        };
        out.push((t, v.to_vec()));
        i += 4 + l;
    }
    out
}

fn av_bytes(pairs: &[(u16, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (t, v) in pairs {
        out.extend_from_slice(&t.to_le_bytes());
        out.extend_from_slice(&(v.len() as u16).to_le_bytes());
        out.extend_from_slice(v);
    }
    out.extend_from_slice(&[0, 0, 0, 0]);
    out
}

/// `NTOWFv2`: HMAC-MD5 over upper-cased user + domain, keyed with the NT hash.
pub fn ntowfv2(password: &str, user: &str, domain: &str) -> [u8; 16] {
    let nt = md4(&utf16le(password));
    hmac_md5(&nt, &utf16le(&format!("{}{}", user.to_uppercase(), domain)))
}

/// The "temp" blob of the NTLMv2 response.
pub fn temp_blob(time: u64, client_challenge: &[u8; 8], target_info: &[u8]) -> Vec<u8> {
    let mut t = vec![1, 1, 0, 0, 0, 0, 0, 0];
    t.extend_from_slice(&time.to_le_bytes());
    t.extend_from_slice(client_challenge);
    t.extend_from_slice(&[0; 4]);
    t.extend_from_slice(target_info);
    t.extend_from_slice(&[0; 4]);
    t
}

/// `(NTProofStr, SessionBaseKey)`.
pub fn proof(ntowf: &[u8; 16], server_challenge: &[u8; 8], temp: &[u8]) -> ([u8; 16], [u8; 16]) {
    let mut data = server_challenge.to_vec();
    data.extend_from_slice(temp);
    let nt_proof = hmac_md5(ntowf, &data);
    (nt_proof, hmac_md5(ntowf, &nt_proof))
}

/// LMv2 response: HMAC over both challenges + the client challenge.
pub fn lmv2_response(
    ntowf: &[u8; 16],
    server_challenge: &[u8; 8],
    client_challenge: &[u8; 8],
) -> [u8; 24] {
    let mut data = server_challenge.to_vec();
    data.extend_from_slice(client_challenge);
    let mac = hmac_md5(ntowf, &data);
    let mut out = [0u8; 24];
    out[..16].copy_from_slice(&mac);
    out[16..].copy_from_slice(client_challenge);
    out
}

/// Windows FILETIME (100 ns since 1601) of a Unix time.
pub fn filetime(unix_secs: u64) -> u64 {
    (unix_secs + 11_644_473_600) * 10_000_000
}

/// Splits `DOMAIN\user` and `user@domain`; an explicit `domain` wins.
pub fn split_account(user: &str, domain: &str) -> (String, String) {
    if !domain.trim().is_empty() {
        return (user.to_string(), domain.trim().to_string());
    }
    if let Some((d, u)) = user.split_once('\\') {
        return (u.to_string(), d.to_string());
    }
    // A UPN (`user@domain`) is sent as it is, with an empty domain.
    (user.to_string(), String::new())
}

/// Inputs of message 3 that are random or time dependent (fixed in tests).
pub struct AuthInput<'a> {
    pub user: &'a str,
    pub domain: &'a str,
    pub password: &'a str,
    pub workstation: &'a str,
    pub client_challenge: [u8; 8],
    /// Used when the server sent no timestamp of its own.
    pub now_filetime: u64,
}

/// Message 3. `negotiate` is message 1 as sent (needed for the MIC).
pub fn authenticate_message(negotiate: &[u8], ch: &Challenge, input: &AuthInput) -> Vec<u8> {
    let (user, domain) = split_account(input.user, input.domain);
    let ntowf = ntowfv2(input.password, &user, &domain);

    let mut pairs = av_pairs(&ch.target_info);
    let ts = pairs
        .iter()
        .find(|(t, _)| *t == AV_TIMESTAMP)
        .and_then(|(_, v)| <[u8; 8]>::try_from(v.as_slice()).ok());
    let use_mic = ts.is_some();
    if use_mic {
        // Announce that the message carries a MIC.
        match pairs.iter_mut().find(|(t, _)| *t == AV_FLAGS) {
            Some((_, v)) if v.len() == 4 => {
                let f = u32::from_le_bytes([v[0], v[1], v[2], v[3]]) | 2;
                *v = f.to_le_bytes().to_vec();
            }
            _ => pairs.push((AV_FLAGS, 2u32.to_le_bytes().to_vec())),
        }
    }
    let time = ts.map_or(input.now_filetime, u64::from_le_bytes);
    let temp = temp_blob(time, &input.client_challenge, &av_bytes(&pairs));
    let (nt_proof, session_key) = proof(&ntowf, &ch.server_challenge, &temp);
    let mut nt_response = nt_proof.to_vec();
    nt_response.extend_from_slice(&temp);
    // With a server timestamp the LM response is all zeros.
    let lm_response: Vec<u8> = if use_mic {
        vec![0; 24]
    } else {
        lmv2_response(&ntowf, &ch.server_challenge, &input.client_challenge).to_vec()
    };

    let flags = OFFERED & ch.flags | NEGOTIATE_UNICODE;
    let header = if use_mic { 88 } else { 72 };
    let mut payload: Vec<u8> = Vec::new();
    let mut fields: Vec<(u16, u32)> = Vec::new();
    for part in [
        lm_response,
        nt_response,
        utf16le(&domain),
        utf16le(&user),
        utf16le(input.workstation),
        Vec::new(), // no session key exchange
    ] {
        fields.push((part.len() as u16, (header + payload.len()) as u32));
        payload.extend_from_slice(&part);
    }

    let mut m = Vec::with_capacity(header + payload.len());
    m.extend_from_slice(SIGNATURE);
    m.extend_from_slice(&3u32.to_le_bytes());
    for (len, off) in &fields {
        m.extend_from_slice(&len.to_le_bytes());
        m.extend_from_slice(&len.to_le_bytes());
        m.extend_from_slice(&off.to_le_bytes());
    }
    m.extend_from_slice(&flags.to_le_bytes());
    m.extend_from_slice(&VERSION);
    let mic_at = m.len();
    if use_mic {
        m.extend_from_slice(&[0; 16]);
    }
    m.extend_from_slice(&payload);
    if use_mic {
        let mut all = negotiate.to_vec();
        all.extend_from_slice(&ch.raw);
        all.extend_from_slice(&m);
        // No key exchange: the exported session key is the session base key.
        let mic = hmac_md5(&session_key, &all);
        m[mic_at..mic_at + 16].copy_from_slice(&mic);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::super::winrm_crypto::hex;
    use super::*;

    const SERVER_CHALLENGE: [u8; 8] = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
    const CLIENT_CHALLENGE: [u8; 8] = [0xaa; 8];

    /// MS-NLMP 4.2.4.1.1: NTOWFv1 of "Password" (the NT hash).
    #[test]
    fn nt_hash_of_the_spec_password() {
        assert_eq!(
            hex(&md4(&utf16le("Password"))),
            "a4f49c406510bdcab6824ee7c30fd852"
        );
    }

    /// MS-NLMP 4.2.4.1.3: NTOWFv2("Password", "User", "Domain").
    #[test]
    fn ntowfv2_vector() {
        assert_eq!(
            hex(&ntowfv2("Password", "User", "Domain")),
            "0c868a403bfd7a93a3001ef22ef02e3f"
        );
    }

    /// MS-NLMP 4.2.4.2.1/2: LMv2 and NTLMv2 responses and the session base key.
    #[test]
    fn ntlmv2_response_vectors() {
        let ntowf = ntowfv2("Password", "User", "Domain");
        let lm = lmv2_response(&ntowf, &SERVER_CHALLENGE, &CLIENT_CHALLENGE);
        assert_eq!(hex(&lm), "86c35097ac9cec102554764a57cccc19aaaaaaaaaaaaaaaa");
        // Target info: MsvAvNbDomainName "Domain", MsvAvNbComputerName "Server", EOL.
        let target = av_bytes(&[(2, utf16le("Domain")), (1, utf16le("Server"))]);
        let temp = temp_blob(0, &CLIENT_CHALLENGE, &target);
        assert_eq!(
            hex(&temp),
            [
                "0101000000000000",
                "0000000000000000",
                "aaaaaaaaaaaaaaaa",
                "00000000",
                "02000c0044006f006d00610069006e00",
                "01000c00530065007200760065007200",
                "00000000",
                "00000000",
            ]
            .concat()
        );
        let (nt_proof, session_key) = proof(&ntowf, &SERVER_CHALLENGE, &temp);
        assert_eq!(hex(&nt_proof), "68cd0ab851e51c96aabc927bebef6a1c");
        assert_eq!(hex(&session_key), "8de40ccadbc14a82f15cb0ad0de95ca3");
    }

    fn challenge_message(info: &[u8], flags: u32) -> Vec<u8> {
        let target = utf16le("SERVER");
        let mut m = Vec::new();
        m.extend_from_slice(SIGNATURE);
        m.extend_from_slice(&2u32.to_le_bytes());
        m.extend_from_slice(&(target.len() as u16).to_le_bytes());
        m.extend_from_slice(&(target.len() as u16).to_le_bytes());
        m.extend_from_slice(&56u32.to_le_bytes());
        m.extend_from_slice(&flags.to_le_bytes());
        m.extend_from_slice(&SERVER_CHALLENGE);
        m.extend_from_slice(&[0; 8]);
        m.extend_from_slice(&(info.len() as u16).to_le_bytes());
        m.extend_from_slice(&(info.len() as u16).to_le_bytes());
        m.extend_from_slice(&((56 + target.len()) as u32).to_le_bytes());
        m.extend_from_slice(&VERSION);
        m.extend_from_slice(&target);
        m.extend_from_slice(info);
        m
    }

    #[test]
    fn negotiate_layout() {
        let m = negotiate_message();
        assert_eq!(&m[..8], b"NTLMSSP\0");
        assert_eq!(u32_at(&m, 8), Some(1));
        assert_eq!(u32_at(&m, 12), Some(OFFERED));
        assert_eq!(m.len(), 40);
    }

    #[test]
    fn challenge_parsing() {
        let info = av_bytes(&[(2, utf16le("DOM")), (7, vec![1, 2, 3, 4, 5, 6, 7, 8])]);
        let msg = challenge_message(&info, OFFERED);
        let ch = parse_challenge(&msg).unwrap();
        assert_eq!(ch.server_challenge, SERVER_CHALLENGE);
        assert_eq!(ch.flags, OFFERED);
        assert_eq!(ch.target_info, info);
        let pairs = av_pairs(&ch.target_info);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[1].0, AV_TIMESTAMP);
        assert!(parse_challenge(b"garbage").is_err());
        assert!(parse_challenge(&negotiate_message()).is_err());
    }

    /// Reads a field of message 3 back.
    fn field(m: &[u8], i: usize) -> &[u8] {
        buffer(m, 12 + i * 8).unwrap()
    }

    #[test]
    fn authenticate_without_timestamp() {
        let info = av_bytes(&[(2, utf16le("Domain")), (1, utf16le("Server"))]);
        let ch = parse_challenge(&challenge_message(&info, OFFERED)).unwrap();
        let neg = negotiate_message();
        let m = authenticate_message(
            &neg,
            &ch,
            &AuthInput {
                user: "User",
                domain: "Domain",
                password: "Password",
                workstation: "COMPUTER",
                client_challenge: CLIENT_CHALLENGE,
                now_filetime: 0,
            },
        );
        assert_eq!(u32_at(&m, 8), Some(3));
        assert_eq!(field(&m, 3), utf16le("User"));
        assert_eq!(field(&m, 2), utf16le("Domain"));
        assert_eq!(field(&m, 4), utf16le("COMPUTER"));
        assert_eq!(field(&m, 5), &[] as &[u8]);
        // Without a timestamp the LMv2 response is sent; the NT response is the proof
        // from the specification followed by the temp blob.
        assert_eq!(
            hex(field(&m, 0)),
            "86c35097ac9cec102554764a57cccc19aaaaaaaaaaaaaaaa"
        );
        let nt = field(&m, 1);
        assert_eq!(hex(&nt[..16]), "68cd0ab851e51c96aabc927bebef6a1c");
        assert_eq!(&nt[16..24], &[1, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(m.len(), 72 + 24 + nt.len() + 12 + 8 + 16);
    }

    #[test]
    fn authenticate_with_timestamp_has_a_valid_mic() {
        let ts = filetime(1_700_000_000).to_le_bytes().to_vec();
        let info = av_bytes(&[(2, utf16le("DOM")), (7, ts.clone())]);
        let ch = parse_challenge(&challenge_message(&info, OFFERED)).unwrap();
        let neg = negotiate_message();
        let input = AuthInput {
            user: "admin",
            domain: "DOM",
            password: "Secret!",
            workstation: "",
            client_challenge: CLIENT_CHALLENGE,
            now_filetime: 5,
        };
        let m = authenticate_message(&neg, &ch, &input);
        // LM response is zeros, the server's timestamp is used and MsvAvFlags bit 2 set.
        assert_eq!(field(&m, 0), &[0u8; 24]);
        let nt = field(&m, 1);
        assert_eq!(&nt[24..32], ts.as_slice());
        let pairs = av_pairs(&nt[44..nt.len() - 4]);
        let flags = pairs.iter().find(|(t, _)| *t == AV_FLAGS).unwrap();
        assert_eq!(flags.1, 2u32.to_le_bytes());
        // Recompute the MIC over negotiate + challenge + message 3 with a zeroed MIC.
        let mut zeroed = m.clone();
        zeroed[72..88].fill(0);
        let mut all = neg.clone();
        all.extend_from_slice(&ch.raw);
        all.extend_from_slice(&zeroed);
        let ntowf = ntowfv2("Secret!", "admin", "DOM");
        let (_, key) = proof(&ntowf, &SERVER_CHALLENGE, &nt[16..]);
        assert_eq!(&m[72..88], &hmac_md5(&key, &all));
        // Proof verification: the server recomputes the same value.
        let (p, _) = proof(&ntowf, &SERVER_CHALLENGE, &nt[16..]);
        assert_eq!(&nt[..16], &p);
    }

    #[test]
    fn account_names() {
        assert_eq!(
            split_account("CORP\\bob", ""),
            ("bob".into(), "CORP".into())
        );
        assert_eq!(
            split_account("bob@corp.example", ""),
            ("bob@corp.example".into(), String::new())
        );
        assert_eq!(split_account("bob", "X"), ("bob".into(), "X".into()));
        assert_eq!(filetime(0), 116_444_736_000_000_000);
    }
}
