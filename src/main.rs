use chrono::Local;
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::Duration;
use winapi::um::winuser::{
    SPI_SETDESKWALLPAPER, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SystemParametersInfoA,
};
use winreg::RegKey;
use winreg::enums::*;

const API_URL: &str = "https://manyacg.top/setu";
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Serialize, Deserialize)]
struct Config {
    download_dir: String,
    change_interval_mins: u64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.contains(&"--uninstall".to_string()) {
        remove_autostart()?;
        println!("Autostart disabled. Program will exit now.");
        return Ok(());
    }
    if args.contains(&"--startup".to_string()) {
        set_autostart()?;

        // 以--startup参数重启程序
        Command::new(&args[0])
            .arg("--startup")
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;
        return Ok(());
    }

    // 获取配置信息
    let config = get_config()?;

    // 确保下载目录存在
    fs::create_dir_all(&config.download_dir)?;

    println!("Wallpaper changer started. Waiting for first change...");

    // 主循环
    loop {
        // 尝试更换壁纸
        match change_wallpaper(&config.download_dir) {
            Ok(_) => println!("Wallpaper changed successfully at {}", Local::now()),
            Err(e) => eprintln!("Error changing wallpaper: {}", e),
        }

        // 等待指定的时间间隔
        let hours = config.change_interval_mins * 60;
        thread::sleep(Duration::from_secs(hours));
    }
}

fn get_config() -> Result<Config, Box<dyn std::error::Error>> {
    let app_data_dir = match env::var("APPDATA") {
        Ok(val) => val,
        Err(_) => {
            // 如果获取APPDATA失败，则使用当前目录
            let mut current_dir = env::current_dir()?;
            current_dir.push("appdata");
            current_dir.to_string_lossy().to_string()
        }
    };

    let config_dir = format!("{}/manyacg-wallpaper", app_data_dir);
    fs::create_dir_all(&config_dir)?;

    let config_path = format!("{}/config.json", config_dir);

    if Path::new(&config_path).exists() {
        let config_str = fs::read_to_string(&config_path)?;
        let config: Config = serde_json::from_str(&config_str)?;
        Ok(config)
    } else {
        // 默认配置
        let default_config = Config {
            download_dir: format!("{}/wallpapers", config_dir),
            change_interval_mins: 60, // 默认每小时更换一次壁纸
        };

        let config_json = serde_json::to_string_pretty(&default_config)?;
        fs::write(&config_path, config_json)?;

        Ok(default_config)
    }
}

fn change_wallpaper(download_dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    // 创建HTTP客户端
    let client = reqwest::blocking::Client::new();

    // 设置请求头
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("ManyACG-Wallpaper-Changer"),
    );

    // 发送请求
    println!("Requesting wallpaper from API...");
    let response = client.get(API_URL).headers(headers).send()?;

    // 获取最终URL（也就是图片直链）
    let final_url = response.url().to_string();
    println!("Got wallpaper URL: {}", final_url);

    // 从URL中提取文件名
    let file_name = match final_url.split('/').last() {
        Some(name) => name,
        _none => "wallpaper.webp",
    };

    // 构建保存路径
    let timestamp = Local::now().format("%Y%m%d%H%M%S");
    let file_path = format!("{}/{}_{}", download_dir, timestamp, file_name);

    // 下载图片
    println!("Downloading wallpaper...");
    let img_bytes = client.get(&final_url).send()?.bytes()?;
    fs::write(&file_path, &img_bytes)?;
    println!("Wallpaper downloaded to: {}", file_path);

    // 设置壁纸
    set_wallpaper(&file_path)?;

    // 删除旧壁纸（保留最新的5个）
    clean_old_wallpapers(download_dir, 5)?;

    Ok(())
}

fn set_wallpaper(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path_cstring = std::ffi::CString::new(path)?;
    let path_ptr = path_cstring.as_ptr();

    unsafe {
        let result = SystemParametersInfoA(
            SPI_SETDESKWALLPAPER,
            0,
            path_ptr as *mut _,
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        );

        if result == 0 {
            return Err("Failed to set wallpaper".into());
        }
    }

    Ok(())
}

fn clean_old_wallpapers(dir: &str, keep_count: usize) -> Result<(), Box<dyn std::error::Error>> {
    // 获取目录中所有文件
    let mut files: Vec<_> = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.path().is_file() && entry.path().extension().map_or(false, |ext| ext == "webp")
        })
        .collect();

    // 如果文件数量少于或等于保留数量，不执行任何操作
    if files.len() <= keep_count {
        return Ok(());
    }

    // 按修改时间排序
    files.sort_by(|a, b| {
        let time_a = a.metadata().unwrap().modified().unwrap();
        let time_b = b.metadata().unwrap().modified().unwrap();
        time_b.cmp(&time_a) // 降序排列，最新的在前面
    });

    // 删除旧文件
    for file in files.iter().skip(keep_count) {
        fs::remove_file(file.path())?;
        println!("Removed old wallpaper: {:?}", file.path());
    }

    Ok(())
}

fn set_autostart() -> Result<(), Box<dyn std::error::Error>> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let path = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
    let (key, _) = hkcu.create_subkey(path)?;

    let exe_path = env::current_exe()?;
    let exe_path_str = exe_path.to_string_lossy().to_string();

    key.set_value(
        "ManyacgWallpaper",
        &format!("\"{}\" --startup", exe_path_str),
    )?;

    println!("Set program to run at startup");
    Ok(())
}

fn remove_autostart() -> Result<(), Box<dyn std::error::Error>> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let path = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
    let key = hkcu.open_subkey_with_flags(path, KEY_WRITE)?;

    // 删除注册表项
    match key.delete_value("ManyacgWallpaper") {
        Ok(_) => println!("Removed program from startup"),
        Err(e) => println!("Failed to remove from startup: {}", e),
    }

    Ok(())
}
