use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// 将 `assets/icons/wnoobar.ico` 零依赖编译为 Win32 资源并嵌入 PE 可执行文件（同时兼容 GNU 与 MSVC 工具链）
fn main() {
    println!("cargo:rerun-if-changed=assets/icons/wnoobar.ico");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let ico_path = manifest_dir.join("assets").join("icons").join("wnoobar.ico");
    if !ico_path.exists() {
        return;
    }

    let ico_bytes = match fs::read(&ico_path) {
        Ok(b) if b.len() >= 6 => b,
        _ => return,
    };

    let count = u16::from_le_bytes([ico_bytes[4], ico_bytes[5]]) as usize;
    if ico_bytes.len() < 6 + count * 16 {
        return;
    }

    // 1. 先在纯内存生成标准 Win32 .res 资源字节流（彻底规避中文路径在 rc/windres 预处理器中的编码问题）
    let mut res = Vec::new();
    write_res_record(&mut res, 0, 0, &[]);

    let mut group_data = Vec::with_capacity(6 + count * 14);
    group_data.extend_from_slice(&ico_bytes[0..6]);

    for i in 0..count {
        let entry_offset = 6 + i * 16;
        let entry = &ico_bytes[entry_offset..entry_offset + 16];
        let bytes_in_res = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
        let image_offset = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;

        let icon_id = (i + 1) as u16;
        group_data.extend_from_slice(&entry[0..12]);
        group_data.extend_from_slice(&icon_id.to_le_bytes());

        if image_offset + bytes_in_res <= ico_bytes.len() {
            let img_data = &ico_bytes[image_offset..image_offset + bytes_in_res];
            write_res_record(&mut res, 3, icon_id, img_data);
        }
    }

    // RT_GROUP_ICON = 14, ID = 1
    write_res_record(&mut res, 14, 1, &group_data);

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let res_path = out_dir.join("wnoobar_icon.res");
    if fs::write(&res_path, &res).is_err() {
        return;
    }

    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env == "gnu" {
        // GNU 工具链：使用 windres 直接将 .res 转换为 COFF .o 对象文件供 ld 链接
        let obj_path = out_dir.join("wnoobar_icon.o");
        let status = Command::new("windres")
            .arg("-J")
            .arg("res")
            .arg("-O")
            .arg("coff")
            .arg("-i")
            .arg(&res_path)
            .arg("-o")
            .arg(&obj_path)
            .status();
        if let Ok(s) = status {
            if s.success() && obj_path.exists() {
                println!("cargo:rustc-link-arg-bins={}", obj_path.display());
            }
        }
    } else {
        // MSVC 工具链：link.exe 原生支持直接链接 .res
        println!("cargo:rustc-link-arg-bins={}", res_path.display());
    }
}

fn write_res_record(buf: &mut Vec<u8>, res_type: u16, res_id: u16, data: &[u8]) {
    let data_size = data.len() as u32;
    let header_size: u32 = 32;

    buf.extend_from_slice(&data_size.to_le_bytes());
    buf.extend_from_slice(&header_size.to_le_bytes());

    if res_type == 0 && res_id == 0 {
        buf.extend_from_slice(&[0xFF, 0xFF, 0x00, 0x00]);
        buf.extend_from_slice(&[0xFF, 0xFF, 0x00, 0x00]);
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        return;
    }

    buf.extend_from_slice(&0xFFFFu16.to_le_bytes());
    buf.extend_from_slice(&res_type.to_le_bytes());
    buf.extend_from_slice(&0xFFFFu16.to_le_bytes());
    buf.extend_from_slice(&res_id.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0x1030u16.to_le_bytes());
    buf.extend_from_slice(&0x0409u16.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(data);

    while buf.len() % 4 != 0 {
        buf.push(0);
    }
}
