//! 对应原项目 `zd_utils.py`：AES-128-CBC 加解密、`getEv` 异或混淆、观看点记录。

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use aes::Aes128;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

pub const IV: &[u8; 16] = b"1g3qqdh4jvbskb9x";
pub const HOME_KEY: &[u8; 16] = b"7q9oko0vqb3la20r";
pub const AI_KEY: &[u8; 16] = b"hw2fdlwcj4cs1mx7";
pub const VIDEO_KEY: &[u8; 16] = b"azp53h0kft7qi78q";
pub const QA_KEY: &[u8; 16] = b"kcGOlISPkYKRksSK";
pub const EXAM_KEY: &[u8; 16] = b"onbfhdyvz8x7otrp";

type Encryptor = cbc::Encryptor<Aes128>;
type Decryptor = cbc::Decryptor<Aes128>;

/// 与 Python 版 `Cipher.encrypt` 完全一致：PKCS7 填充 + AES-128-CBC + Base64。
pub fn encrypt(key: &[u8; 16], data: &str) -> String {
    let ct = Encryptor::new(key.into(), IV.into())
        .encrypt_padded_vec_mut::<Pkcs7>(data.as_bytes());
    B64.encode(ct)
}

/// 与 Python 版 `Cipher.decrypt` 一致；返回 UTF-8 文本（非法字节用替换字符兜底）。
pub fn decrypt(key: &[u8; 16], b64_text: &str) -> anyhow::Result<String> {
    let raw = B64
        .decode(b64_text.trim())
        .map_err(|e| anyhow::anyhow!("base64 解码失败: {e}"))?;
    let pt = Decryptor::new(key.into(), IV.into())
        .decrypt_padded_vec_mut::<Pkcs7>(&raw)
        .map_err(|e| anyhow::anyhow!("AES 解密失败: {e}"))?;
    Ok(String::from_utf8_lossy(&pt).into_owned())
}

/// `getEv`：把字段用 `;` 拼起来，逐字符与循环密钥异或，取十六进制低 4 位拼接。
pub fn get_ev<T: ToString>(data: &[T], key: &str) -> String {
    let joined = data
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(";");
    let mut key_iter = key.bytes().cycle();
    let mut out = String::with_capacity(joined.len() * 4);
    for ch in joined.chars() {
        let k = key_iter.next().unwrap_or(b'z') as u32;
        let x = (ch as u32) ^ k;
        let hexed = format!("{x:x}");
        // Python: tmp[-4:] —— 不足 4 位时先补到 2 位再取后 4 位，效果等价
        let padded = if hexed.len() < 2 {
            format!("0{hexed}")
        } else {
            hexed
        };
        if padded.len() >= 4 {
            out.push_str(&padded[padded.len() - 4..]);
        } else {
            out.push_str(&padded);
        }
    }
    out
}

/// 观看进度点，对应 Python `WatchPoint`。
#[derive(Debug, Clone)]
pub struct WatchPoint {
    points: Vec<i64>,
    last: i64,
}

impl Default for WatchPoint {
    fn default() -> Self {
        Self::new(0)
    }
}

impl WatchPoint {
    pub fn new(init: i64) -> Self {
        let mut s = Self {
            points: Vec::new(),
            last: 0,
        };
        s.reset(init);
        s
    }

    /// 每 2 秒记录一个点，`start` 省略时从上次位置续。
    pub fn add(&mut self, end: f64, start: Option<i64>) {
        const INTERVAL: i64 = 2;
        let start = start.unwrap_or(self.last);
        let end = end as i64;
        self.last = end;
        let mut i = start;
        while i <= end {
            self.points.push(Self::gen(i));
            i += INTERVAL;
        }
    }

    pub fn get(&self) -> String {
        self.points
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }

    pub fn reset(&mut self, init: i64) {
        self.points = vec![0, 1];
        self.last = if init > 0 { init } else { 1 };
    }

    fn gen(time: i64) -> i64 {
        time / 5 + 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cipher_roundtrip() {
        let text = r#"{"recruitAndCourseId":"abc123","pageNo":1}"#;
        let enc = encrypt(VIDEO_KEY, text);
        let dec = decrypt(VIDEO_KEY, &enc).unwrap();
        assert_eq!(dec, text);
    }

    #[test]
    fn ev_matches_python_shape() {
        // getEv([1,2,3])：拼成 "1;2;3"，与 "zzpttjd" 循环异或后取十六进制
        // 0x4b 0x41 0x42 0x4f 0x47
        let ev = get_ev(&[1, 2, 3], "zzpttjd");
        assert_eq!(ev, "4b41424f47");
    }

    #[test]
    fn hms_format() {
        assert_eq!(crate::util::hms(30.0), "0:00:30");
        assert_eq!(crate::util::hms(3_661.0), "1:01:01");
    }
}
