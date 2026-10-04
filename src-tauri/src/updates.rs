//! GitHub 正式发布检查。仅访问本应用的官方仓库，不使用文档预览的网络权限。
use semver::Version;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const API: &str = "https://api.github.com/repos/eight8xx/personal-document-manager/releases/latest";
const RELEASE_PREFIX: &str = "https://github.com/eight8xx/personal-document-manager/releases/";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    body: Option<String>,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    current_version: String,
    update: Option<AvailableUpdate>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AvailableUpdate {
    version: String,
    notes: String,
    release_url: String,
    download_url: Option<String>,
}

fn parse_release(current: &str, release: Release) -> Result<UpdateCheck, String> {
    let current_version = Version::parse(current).map_err(|_| "当前应用版本无效")?;
    let version = Version::parse(release.tag_name.trim_start_matches('v'))
        .map_err(|_| "GitHub 发布版本号无效")?;
    let mut check = UpdateCheck {
        current_version: current.to_owned(),
        update: None,
    };
    if release.draft || release.prerelease || !version.pre.is_empty() || version <= current_version
    {
        return Ok(check);
    }
    if !release
        .html_url
        .starts_with(&format!("{RELEASE_PREFIX}tag/"))
    {
        return Err("GitHub 发布地址不属于本应用".into());
    }
    let download_url = release
        .assets
        .into_iter()
        .find(|asset| {
            let name = asset.name.to_ascii_lowercase();
            name.contains("-setup-")
                && name.ends_with(".exe")
                && asset
                    .browser_download_url
                    .starts_with(&format!("{RELEASE_PREFIX}download/"))
        })
        .map(|asset| asset.browser_download_url);
    check.update = Some(AvailableUpdate {
        version: version.to_string(),
        notes: release.body.unwrap_or_default(),
        release_url: release.html_url,
        download_url,
    });
    Ok(check)
}

#[tauri::command]
pub async fn check_app_update(app: tauri::AppHandle) -> Result<UpdateCheck, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!(
            "personal-document-manager/",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .map_err(|_| "无法初始化更新检查")?;
    let response = client
        .get(API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|_| "无法连接 GitHub，请检查网络后重试")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(UpdateCheck {
            current_version: app.package_info().version.to_string(),
            update: None,
        });
    }
    if response.status() == reqwest::StatusCode::FORBIDDEN
        || response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        return Err("GitHub 请求过于频繁，请稍后重试".into());
    }
    let release = response
        .error_for_status()
        .map_err(|_| "GitHub 更新服务暂时不可用")?
        .json::<Release>()
        .await
        .map_err(|_| "无法读取 GitHub 发布信息")?;
    parse_release(&app.package_info().version.to_string(), release)
}

#[tauri::command]
pub async fn open_app_update(app: tauri::AppHandle) -> Result<(), String> {
    // 点击时重新验证正式发布，前端不能传入任意下载地址。
    let check = check_app_update(app).await?;
    let update = check.update.ok_or("当前没有可用更新")?;
    let url = update.download_url.unwrap_or(update.release_url);
    tauri::async_runtime::spawn_blocking(move || {
        open::that(url).map_err(|_| "无法打开浏览器，请稍后重试".to_owned())
    })
    .await
    .map_err(|_| "无法打开更新下载".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;
    fn release(version: &str) -> Release {
        Release {
            tag_name: version.into(),
            html_url: format!("{RELEASE_PREFIX}tag/{version}"),
            body: Some("修复与改进".into()),
            draft: false,
            prerelease: false,
            assets: vec![],
        }
    }
    #[test]
    fn compares_semver_and_ignores_unstable_or_older_versions() {
        assert!(parse_release("0.1.9", release("v0.1.10"))
            .unwrap()
            .update
            .is_some());
        for version in ["v0.1.9", "v0.1.8", "v0.2.0-beta.1"] {
            assert!(parse_release("0.1.9", release(version))
                .unwrap()
                .update
                .is_none());
        }
        let mut draft = release("v0.2.0");
        draft.draft = true;
        assert!(parse_release("0.1.0", draft).unwrap().update.is_none());
        let mut preview = release("v0.2.0");
        preview.prerelease = true;
        assert!(parse_release("0.1.0", preview).unwrap().update.is_none());
        assert!(parse_release("0.1.0", release("invalid")).is_err());
    }
    #[test]
    fn only_official_installer_assets_are_downloaded() {
        let mut next = release("v0.2.0");
        next.assets = vec![
            Asset {
                name: "app-Setup-0.2.0.exe".into(),
                browser_download_url: "https://example.com/app.exe".into(),
            },
            Asset {
                name: "app-Portable.exe".into(),
                browser_download_url: format!("{RELEASE_PREFIX}download/v0.2.0/portable.exe"),
            },
            Asset {
                name: "app-Setup-0.2.0.exe".into(),
                browser_download_url: format!("{RELEASE_PREFIX}download/v0.2.0/setup.exe"),
            },
        ];
        let update = parse_release("0.1.0", next).unwrap().update.unwrap();
        assert_eq!(
            update.download_url,
            Some(format!("{RELEASE_PREFIX}download/v0.2.0/setup.exe"))
        );
        let mut invalid = release("v0.2.0");
        invalid.html_url = "https://example.com".into();
        assert!(parse_release("0.1.0", invalid).is_err());
        assert!(parse_release("0.1.0", release("v0.2.0"))
            .unwrap()
            .update
            .unwrap()
            .download_url
            .is_none());
    }
}
