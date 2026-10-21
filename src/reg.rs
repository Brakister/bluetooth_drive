// Camada fina sobre a Win32 Registry API.

use windows_sys::Win32::System::Registry::*;

use crate::util::wide;

pub const ERR_SUCCESS: u32 = 0;
pub const ERR_FILE_NOT_FOUND: u32 = 2;

pub struct Key(pub HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

pub fn open(path: &str, access: u32) -> Result<Key, String> {
    // RegOpenKeyExW trabalha com caminhos RELATIVOS a uma raiz. Aceitamos o
    // prefixo "HKLM\" por convencao e o removemos aqui.
    let sub = path
        .strip_prefix(r"HKLM\")
        .or_else(|| path.strip_prefix(r"HKEY_LOCAL_MACHINE\"))
        .unwrap_or(path);
    let mut h: HKEY = std::ptr::null_mut();
    let r = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, wide(sub).as_ptr(), 0, access, &mut h) };
    if r != ERR_SUCCESS {
        return Err(format!("RegOpenKeyExW(\"{sub}\") falhou: {r}"));
    }
    Ok(Key(h))
}

pub fn open_read(path: &str) -> Result<Key, String> {
    open(path, KEY_READ)
}

pub fn open_write(path: &str) -> Result<Key, String> {
    // KEY_SET_VALUE para gravar + KEY_READ para ler o valor original (backup).
    open(path, KEY_SET_VALUE | KEY_QUERY_VALUE | KEY_READ)
}

pub fn read_bytes(key: &Key, name: &str) -> Result<Option<Vec<u8>>, String> {
    let mut dtype: u32 = 0;
    let mut size: u32 = 0;
    let r = unsafe {
        RegQueryValueExW(
            key.0,
            wide(name).as_ptr(),
            std::ptr::null_mut(),
            &mut dtype,
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if r == ERR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if r != ERR_SUCCESS {
        return Err(format!("RegQueryValueExW(\"{name}\") falhou: {r}"));
    }
    let mut buf = vec![0u8; size.max(1) as usize];
    let r = unsafe {
        RegQueryValueExW(
            key.0,
            wide(name).as_ptr(),
            std::ptr::null_mut(),
            &mut dtype,
            buf.as_mut_ptr(),
            &mut size,
        )
    };
    if r != ERR_SUCCESS {
        return Err(format!("RegQueryValueExW(\"{name}\") falhou: {r}"));
    }
    buf.truncate(size as usize);
    Ok(Some(buf))
}

pub fn read_dword(key: &Key, name: &str) -> Result<Option<u32>, String> {
    match read_bytes(key, name)? {
        None => Ok(None),
        Some(b) if b.len() >= 4 => Ok(Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))),
        Some(_) => Ok(None),
    }
}

pub fn read_string(key: &Key, name: &str) -> Result<Option<String>, String> {
    match read_bytes(key, name)? {
        None => Ok(None),
        Some(b) => {
            let mut v = Vec::with_capacity(b.len() / 2);
            let mut i = 0;
            while i + 1 < b.len() {
                v.push(u16::from_le_bytes([b[i], b[i + 1]]));
                i += 2;
            }
            Ok(Some(String::from_utf16_lossy(&v)))
        }
    }
}

pub fn write_bytes(key: &Key, name: &str, data: &[u8], vtype: u32) -> Result<(), String> {
    let r = unsafe {
        RegSetValueExW(
            key.0,
            wide(name).as_ptr(),
            0,
            vtype,
            data.as_ptr(),
            data.len() as u32,
        )
    };
    if r != ERR_SUCCESS {
        return Err(format!("RegSetValueExW(\"{name}\") falhou: {r}"));
    }
    Ok(())
}

pub fn write_dword(key: &Key, name: &str, v: u32) -> Result<(), String> {
    write_bytes(key, name, &v.to_le_bytes(), REG_DWORD)
}

pub fn delete_value(key: &Key, name: &str) -> Result<bool, String> {
    let r = unsafe { RegDeleteValueW(key.0, wide(name).as_ptr()) };
    if r == ERR_SUCCESS {
        Ok(true)
    } else if r == ERR_FILE_NOT_FOUND {
        Ok(false)
    } else {
        Err(format!("RegDeleteValueW(\"{name}\") falhou: {r}"))
    }
}

pub fn enum_subkeys(key: &Key) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut idx: u32 = 0;
    loop {
        let mut len: u32 = 256;
        let mut buf = vec![0u16; len as usize];
        let r = unsafe {
            RegEnumKeyExW(
                key.0,
                idx,
                buf.as_mut_ptr(),
                &mut len,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if r != ERR_SUCCESS {
            break;
        }
        out.push(String::from_utf16_lossy(&buf[..len as usize]));
        idx += 1;
        if idx > 4096 {
            break;
        }
    }
    Ok(out)
}
