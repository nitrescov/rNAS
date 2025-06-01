/*
This file is compiled to the server binary. It contains all important functions of the server.
Copyright (C) 2023  Nico Pieplow (nitrescov)
Contact: nitrescov@protonmail.com

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU Affero General Public License as published
by the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU Affero General Public License for more details.

You should have received a copy of the GNU Affero General Public License
along with this program.  If not, see <https://www.gnu.org/licenses/>.
*/

#[macro_use] extern crate rocket;
#[macro_use] extern crate lazy_static;

use std::env;
use std::thread;
use std::ffi::OsStr;
use std::fmt::Debug;
use std::time::Duration;
use std::string::String;
use std::process::Command;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf, MAIN_SEPARATOR_STR};
use std::fs::{File, read_to_string, remove_file, remove_dir_all, create_dir};
use md5::Md5;
use sha2::{Sha384, Digest};
use serde::{Deserialize, Serialize};
use rocket::form::Form;
use rocket::response::Redirect;
use rocket::request::FromSegments;
use rocket::http::{Cookie, CookieJar};
use rocket::response::content::RawHtml;
use rocket::{Rocket, Build, FromForm, Either};
use rocket::fs::{FileServer, NamedFile, TempFile};
use rocket::http::uri::{Segments, error::PathError};
use rocket::http::uri::fmt::{FromUriParam, Path as RocketPath};

pub struct DotPathBuf(PathBuf);

impl FromSegments<'_> for DotPathBuf {
    type Error = PathError;

    fn from_segments(segments: Segments<'_, RocketPath>) -> Result<Self, Self::Error> {
        match segments.to_path_buf(true) {
            Ok(p) => Ok(DotPathBuf(p)),
            Err(e) => Err(e),
        }
    }
}

impl<'a> FromUriParam<RocketPath, &'a str> for DotPathBuf {
    type Target = &'a Path;

    #[inline(always)]
    fn from_uri_param(param: &'a str) -> &'a Path {
        Path::new(param)
    }
}

#[derive(Serialize, Deserialize, Debug)]
struct Config {
    language: String,
    storage_path: String,
    owner: String,
    background: String,
    foreground: String,
    accent_background: String,
    accent_foreground: String,
    shadows: String,
    errors: String,
    input: String,
    clean_tmp_files: u64,
    whitelist: String,
    name_length: usize,
}

#[derive(FromForm)]
struct LoginData {
    name: String,
    password: String,
}

#[derive(FromForm)]
struct FolderName {
    folder_name: String,
}

#[derive(FromForm)]
struct ArchiveName {
    archive_name: String,
}

#[derive(FromForm)]
struct Upload<'r> {
    file: TempFile<'r>,
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

lazy_static! {
    // Load the config file
    static ref CONFIG: Config = toml::from_str(
        read_to_string("config.toml").expect("Cannot access config file").as_str()
    ).expect("Config file formatting is invalid");

    // Ensure a canonical, absolute storage path
    static ref STORAGE: PathBuf = if CONFIG.storage_path.len() == 0 {
        env::current_dir().expect("Cannot access the current working directory")
    } else if &CONFIG.storage_path[0..1] != "/" {
        env::current_dir().expect("Cannot access the current working directory").join(CONFIG.storage_path.as_str())
    } else {
        PathBuf::from(CONFIG.storage_path.as_str())
    };

    // Apply the settings to the static HTML files
    static ref HOME: String = load_static_file("home");
    static ref LOGIN_FAILED: String = load_static_file("login_failed");
    static ref ACCESS_DENIED: String = load_static_file("access_denied");
    static ref NO_DIRECTORY: String = load_static_file("no_directory");
    static ref NO_FILE: String = load_static_file("no_file");
    static ref IS_DIRECTORY: String = load_static_file("is_directory");
    static ref IS_FILE: String = load_static_file("is_file");
    static ref UPLOAD_ERROR: String = load_static_file("upload_error");
    static ref UNPACK_ERROR: String = load_static_file("unpack_error");
}

fn load_static_file(input: &str) -> String {
    let tmp = read_to_string(format!("static/{}_{}.html", CONFIG.language, input)).expect("Cannot read static HTML file");
    tmp
        .replace("{{OW}}", CONFIG.owner.as_str())
        .replace("{{BG}}", CONFIG.background.as_str())
        .replace("{{FG}}", CONFIG.foreground.as_str())
        .replace("{{ABG}}", CONFIG.accent_background.as_str())
        .replace("{{AFG}}", CONFIG.accent_foreground.as_str())
        .replace("{{SH}}", CONFIG.shadows.as_str())
        .replace("{{ER}}", CONFIG.errors.as_str())
        .replace("{{IN}}", CONFIG.input.as_str())
}

fn get_users() -> Vec<(String, String)> {
    let file = File::open("users.csv").expect("Cannot read users file");
    let buffer = BufReader::new(file);
    buffer.lines().map(|line| {
        match line.expect("Cannot parse line in users file").split_once(";") {
            None => ("".to_owned(), "".to_owned()),
            Some(tuple) => (tuple.0.to_owned(), tuple.1.to_owned())
        }
    }).collect()
}

fn check_login(cookies: &CookieJar<'_>, path: &PathBuf) -> Option<String> {
    let mut username = String::new();
    for item in path.iter() {
        if item != OsStr::new(&MAIN_SEPARATOR_STR) {
            username.push_str(item.to_str().expect("Invalid path encoding (expected UTF-8)"));
            break;
        }
    }
    let hash_value = match cookies.get_private("user_hash") {
        None => return None,
        Some(cookie) => cookie.value().to_owned()
    };
    for entry in get_users() {
        if hash_value == entry.0 && username == entry.1 { return Some(username) }
    }
    None
}

fn check_path(path: &PathBuf) -> (bool, bool) {
    let full_path = STORAGE.join(&path);
    (full_path.is_file(), full_path.is_dir())
}

fn sanitize_string(input: &str) -> String {
    let mut temp_string = String::new();
    // Check whether the input string contains any characters not listed in the whitelist and add all valid characters to the return string
    for c in input.chars() {
        if !CONFIG.whitelist.chars().all(|w| w != c) {
            temp_string.push(c);
        }
    }
    // Remove leading and trailing whitespaces or dots
    while temp_string.starts_with(" ") {
        temp_string = temp_string[1..].to_owned();
    }
    while temp_string.ends_with(" ") || temp_string.ends_with(".") {
        temp_string = temp_string[..temp_string.len() - 1].to_owned();
    }
    // Restrict the string length as specified in the config
    if temp_string.len() > CONFIG.name_length {
        temp_string = temp_string[temp_string.len() - CONFIG.name_length..].to_owned();
    }
    temp_string
}

#[get("/")]
fn home() -> RawHtml<String> { RawHtml(HOME.to_owned()) }

#[get("/favicon.ico")]
async fn favicon() -> Option<NamedFile> { NamedFile::open(Path::new("icons/favicon.ico")).await.ok() }

#[post("/", data = "<data>")]
fn login(cookies: &CookieJar<'_>, data: Option<Form<LoginData>>) -> Either<Redirect, RawHtml<String>> {
    match data {
        None => Either::Right(RawHtml(LOGIN_FAILED.to_owned())),
        Some(login_data) => {
            let hash_value = Sha384::digest(format!("{}{}", login_data.password, login_data.name));
            for entry in get_users() {
                if format!("{:x}", hash_value) == entry.0 {
                    let mut cookie = Cookie::new("user_hash", format!("{:x}", hash_value));
                    cookie.set_expires(None);
                    cookies.add_private(cookie);
                    return Either::Left(Redirect::to(uri!(list_directory(&login_data.name))))
                }
            }
            Either::Right(RawHtml(LOGIN_FAILED.to_owned()))
        }
    }
}

#[get("/files/<path..>")]
fn list_directory(cookies: &CookieJar<'_>, path: DotPathBuf) -> RawHtml<String> {
    let path = path.0;
    if let Some(username) = check_login(cookies, &path) {
        if check_path(&path).1 {

            // Determine the path string and the parent directory
            let path_string = path.to_str().expect("Invalid path encoding (expected UTF-8)");
            let parent_path = match path.parent() {
                None => String::new(),
                Some(parent) => match parent.to_str().expect("Invalid path encoding (expected UTF-8)") {
                    "" => String::new(),
                    path_string => "files/".to_owned() + path_string
                }
            };

            // Create the top navigation bar
            let mut current_link = "/files".to_owned();
            let mut top_bar = String::new();
            for part in path_string.split("/") {
                current_link.push_str(format!("/{0}", part).as_str());
                top_bar.push_str(format!("/ <a href=\"{0}\" style=\"color:{1};\">{2}</a> ", current_link, CONFIG.accent_foreground, part).as_str());
            }

            // Get and sort the files and subdirectories from the given path
            let mut files = Vec::new();
            let mut directories = Vec::new();
            for item in STORAGE.join(&path).read_dir().expect("Cannot read directory contents") {
                if let Ok(item) = item {
                    match item.path().file_name() {
                        None => {},
                        Some(name) => {
                            if item.path().is_file() { files.push(name.to_owned()) }
                            else if item.path().is_dir() { directories.push(name.to_owned()) }
                        }
                    }
                }
            }
            files.sort_by_key(|k| k.to_ascii_lowercase());
            directories.sort_by_key(|k| k.to_ascii_lowercase());

            // Configure translatable messages and texts
            let mut del_dir = "The directory will be deleted permanently. Continue?";
            let mut del_file = "The file will be deleted permanently. Continue?";
            let mut menu_content: (&str, &str, &str, &str, &str, &str, &str, &str, &str, &str, &str, &str, &str) =
                ("Files from", "Home directory", "Parent directory", "Download directory (ZIP)", "Create directory",
                 "Unpack ZIP file", "Upload file", "directory_name", "file_name.zip", "folder(s)", "file(s)", "version", "disk usage");
            if CONFIG.language == "de" {
                del_dir = "Der Ordner wird endgültig gelöscht. Fortfahren?";
                del_file = "Die Datei wird endgültig gelöscht. Fortfahren?";
                menu_content = ("Dateien von", "Hauptverzeichnis", "Übergeordnetes Verzeichnis", "Ordner herunterladen (ZIP)", "Ordner erstellen",
                                "ZIP-Datei entpacken", "Datei hochladen", "Ordnername", "Dateiname.zip", "Ordner", "Datei(en)", "Version", "Festplattennutzung");
            }

            // Create the directory list
            let mut dir_list = String::new();
            for dir in &directories {
                dir_list.push_str(format!(
                    "<div style=\"display:inline-block; padding:8px; border-bottom-style:solid; border-width:1px; border-color:{3}\"> \
                        <a href=\"/files/{0}/{1}\" style=\"text-decoration:none; display:inline-block\"> \
                            <div style=\"font-family:sans-serif; font-size:14px; text-align:left; color:{2}; vertical-align:middle; width: 500px\"> \
                                <img src=\"/icons/folder_32x32.png\" style=\"vertical-align:middle; margin-right:8px\"/> \
                                {1} </div></a> \
                        <a href=\"/delete_dir/{0}/{1}\" onclick=\"return confirm(\'{4}\');\" style=\"text-decoration:none; display:inline-block\"> \
                            <div style=\"vertical-align:middle; width:32px\"> \
                                <img src=\"/icons/trash_16x16.png\" style=\"vertical-align:middle\"/> \
                    </div></a></div><br>",
                    path_string, dir.to_str().expect("Invalid path encoding (expected UTF-8)"), CONFIG.foreground, CONFIG.shadows, del_dir
                ).as_str())
            }

            // Create the file list
            let mut file_list = String::new();
            for file in &files {
                let file_extension = match Path::new(&file).extension() {
                    None => "".to_owned(),
                    Some(ext) => ext.to_str().expect("Cannot extract file extension").to_lowercase()
                };
                let file_type = match file_extension.as_str() {
                    "png" | "bmp" | "jpg" | "jpeg" | "gif" | "tga" | "dds" | "heic" | "webp" | "tif" | "tiff" | "ico" => "image",
                    "zip" | "rar" | "tar" | "7z" | "gz" | "xz" | "z" | "deb" | "rpm" => "archive",
                    "mkv" | "webm" | "flv" | "avi" | "mov" | "wmv" | "mp4" | "m4v" | "mpg" | "mpeg" => "video",
                    "aac" | "mp3" | "m4a" | "acc" | "wav" | "wma" | "ogg" | "flac" | "aiff" | "alac" | "dsd" | "mqa" | "opus" => "music",
                    "c" | "cgi" | "pl" | "class" | "cpp" | "cs" | "h" | "java" | "php" | "html" | "css" | "py" | "swift" | "vb" | "rs" => "code",
                    "exe" | "msi" | "apk" | "bat" | "bin" | "com" | "jar" | "ps1" | "sh" => "executable",
                    "pdf" => "pdf",
                    _ => "file"
                };
                file_list.push_str(format!(
                    "<div style=\"display:inline-block; padding:8px; border-bottom-style:solid; border-width:1px; border-color:{3}\"> \
                        <a href=\"/download/{0}/{1}\" style=\"text-decoration:none; display:inline-block\"> \
                            <div style=\"font-family:sans-serif; font-size:14px; text-align:left; color:{2}; vertical-align:middle; width: 500px\"> \
                                <img src=\"/icons/{5}_32x32.png\" style=\"vertical-align:middle; margin-right:8px\"/> \
                                {1} </div></a> \
                        <a href=\"/delete_file/{0}/{1}\" onclick=\"return confirm(\'{4}\');\" style=\"text-decoration:none; display:inline-block\"> \
                            <div style=\"vertical-align:middle; width:32px\"> \
                                <img src=\"/icons/trash_16x16.png\" style=\"vertical-align:middle\"/> \
                    </div></a></div><br>",
                    path_string, file.to_str().expect("Invalid path encoding (expected UTF-8)"), CONFIG.foreground, CONFIG.shadows, del_file, file_type
                ).as_str())
            }

            // Get the disk usage of the storage filesystem (Linux only)
            let storage_cmd = Command::new("df")
                .arg(STORAGE.join(&path))
                .arg("--output=pcent")
                .output()
                .expect("Cannot execute df command")
                .stdout;
            let mut percent = String::new();
            for byte in storage_cmd.into_iter() {
                match byte {
                    48u8..=57u8 => percent.push(byte as char),
                    _ => continue
                }
            }

            // Create the HTML page with top and bottom bars
            let directory_view = format!(
                "<!DOCTYPE html> \
                <html lang=\"{0}\"> \
                <head> \
                    <meta charset=\"utf-8\"> \
                    <title>{8} {20}</title> \
                </head> \
                <body style=\"background-color:{1}; margin-top:0px\"> \
                <div style=\"background-color:{1}; position:sticky; top:0px; width:100%; padding-top:16px; padding-bottom:8px\"> \
                    <h1 style=\"font-family:sans-serif; font-size:24px; text-align:center; font-weight:bold; color:{2}; background-color:{3}; \
                            border-radius:10px; margin:16px; margin-top:0px; margin-bottom:8px; padding:8px; box-shadow:2px 2px 4px {4}\"> \
                        {26} \
                    </h1> \
                    <div style=\"text-align:center\"> \
                        <form action=\"/files/{20}\" style=\"margin:8px; display:inline-block\"> \
                            <input value=\"{9}\" type=\"submit\" style=\"font-family:sans-serif; font-size:14px; text-align:left; width:250px; \
                            color:{2}; background:{3} url(\'/icons/home_16x16.png\') no-repeat scroll 10px; \
                            border-radius:4px; border-style:hidden; padding:8px; padding-left:36px; cursor:pointer; box-shadow:2px 2px 4px {4}\" /> \
                        </form> \
                        <form action=\"/{27}\" style=\"margin:8px; display:inline-block\"> \
                            <input value=\"{10}\" type=\"submit\" style=\"font-family:sans-serif; font-size:14px; text-align:left; width:250px; \
                            color:{2}; background:{3} url(\'/icons/back_16x16.png\') no-repeat scroll 10px; \
                            border-radius:4px; border-style:hidden; padding:8px; padding-left:36px; cursor:pointer; box-shadow:2px 2px 4px {4}\" /> \
                        </form> \
                        <form action=\"/zip/{28}\" style=\"margin:8px; display:inline-block\"> \
                            <input value=\"{11}\" type=\"submit\" style=\"font-family:sans-serif; font-size:14px; text-align:left; width:250px; \
                            color:{2}; background:{3} url(\'/icons/download_16x16.png\') no-repeat scroll 10px; \
                            border-radius:4px; border-style:hidden; padding:8px; padding-left:36px; cursor:pointer; box-shadow:2px 2px 4px {4}\" /> \
                        </form> \
                    </div> \
                    <div style=\"text-align:center\"> \
                        <form action=\"/new_dir/{28}\" method=\"post\" style=\"margin:8px; display:inline-block\"> \
                            <input value=\"{12}\" type=\"submit\" style=\"font-family:sans-serif; font-size:14px; text-align:left; width:250px; \
                            color:{2}; background:{3} url(\'/icons/folder_16x16.png\') no-repeat scroll 10px; \
                            border-radius:4px; border-style:hidden; padding:8px; padding-left:36px; cursor:pointer; box-shadow:2px 2px 4px {4}\" /> \
                            <br> \
                            <input name=\"folder_name\" type=\"text\" style=\"font-family:sans-serif; font-size:14px; text-align:left; width:234px; \
                            color:{6}; background-color:{7}; border-radius:4px; border-style:hidden; padding:8px; margin-top:8px\" \
                            placeholder=\"{15}\" required /> \
                        </form> \
                        <form action=\"/unpack/{28}\" method=\"post\" style=\"margin:8px; display:inline-block\"> \
                            <input value=\"{13}\" type=\"submit\" style=\"font-family:sans-serif; font-size:14px; text-align:left; width:250px; \
                            color:{2}; background:{3} url(\'/icons/archive_16x16.png\') no-repeat scroll 10px; \
                            border-radius:4px; border-style:hidden; padding:8px; padding-left:36px; cursor:pointer; box-shadow:2px 2px 4px {4}\" /> \
                            <br> \
                            <input name=\"archive_name\" type=\"text\" style=\"font-family:sans-serif; font-size:14px; text-align:left; width:234px; \
                            color:{6}; background-color:{7}; border-radius:4px; border-style:hidden; padding:8px; margin-top:8px\" \
                            placeholder=\"{16}\" required /> \
                        </form> \
                        <form action=\"/upload/{28}\" method=\"post\" style=\"margin:8px; display:inline-block\" enctype=\"multipart/form-data\"> \
                            <input value=\"{14}\" type=\"submit\" style=\"font-family:sans-serif; font-size:14px; text-align:left; width:250px; \
                            color:{2}; background:{3} url(\'/icons/upload_16x16.png\') no-repeat scroll 10px; \
                            border-radius:4px; border-style:hidden; padding:8px; padding-left:36px; cursor:pointer; box-shadow:2px 2px 4px {4}\" /> \
                            <br> \
                            <input name=\"file\" type=\"file\" style=\"font-family:sans-serif; font-size:14px; text-align:left; width:240px; \
                            color:{6}; background-color:{7}; border-radius:4px; border-style:hidden; padding:5px; margin-top:8px\" required /> \
                        </form> \
                    </div> \
                </div> \
                <div style=\"text-align:center\"> \
                    {21}<br><br> \
                    {22}<br><br> \
                </div> \
                <div style=\"margin:auto; border-radius:4px; border-style:hidden; width:270px; height:6px; \
                background:linear-gradient(to right, {4} 0%, {4} {29}%, {7} {29}%, {7} 100%)\"></div><br> \
                <p style=\"margin:auto; font-family:sans-serif; font-size:14px; text-align:center; color:{6}\"> \
                    {23} {17}, {24} {18} &ensp; | &ensp; {29}% {30} \
                </p><br><br> \
                <p style=\"margin:auto; font-family:sans-serif; font-size:12px; text-align:center; color:{6}; \
                border-top-style:solid; border-color:{4}; border-width:1px; width:250px; padding:10px\"> \
                    - {5} rNAS {19} {25} - \
                </p> \
                </body> \
                </html>",
                CONFIG.language, CONFIG.background, CONFIG.accent_foreground, CONFIG.accent_background, CONFIG.shadows, CONFIG.owner, CONFIG.foreground, CONFIG.input,
                menu_content.0, menu_content.1, menu_content.2, menu_content.3, menu_content.4, menu_content.5, menu_content.6, menu_content.7, menu_content.8,
                menu_content.9, menu_content.10, menu_content.11, username, dir_list, file_list, directories.len(), files.len(), VERSION,
                top_bar, parent_path, path_string, percent, menu_content.12
            );

            RawHtml(directory_view)
        }
        else { RawHtml(NO_DIRECTORY.to_owned()) }
    }
    else { RawHtml(ACCESS_DENIED.to_owned()) }
}

#[get("/download/<path..>")]
async fn download_file(cookies: &CookieJar<'_>, path: DotPathBuf) -> Either<Option<NamedFile>, RawHtml<String>> {
    let path = path.0;
    if let Some(_username) = check_login(cookies, &path) {
        if check_path(&path).0 {
            Either::Left(NamedFile::open(STORAGE.join(&path)).await.ok())
        }
        else { Either::Right(RawHtml(NO_FILE.to_owned())) }
    }
    else { Either::Right(RawHtml(ACCESS_DENIED.to_owned())) }
}

#[get("/zip/<path..>")]
async fn download_folder(cookies: &CookieJar<'_>, path: DotPathBuf) -> Either<Option<NamedFile>, RawHtml<String>> {
    let path = path.0;
    if let Some(_username) = check_login(cookies, &path) {
        if check_path(&path).1 {
            let hash_value = format!("{:x}", Md5::digest(path.to_str().expect("Invalid path encoding (expected UTF-8)")));
            let directory_name = path
                .file_name().expect("Cannot extract directory name")
                .to_str().expect("Invalid directory name encoding (expected UTF-8)");
            let file_name = directory_name.to_owned() + "-" + &hash_value + ".zip";
            let temp_file_path = STORAGE.join("tmp").join(&file_name);
            if temp_file_path.is_file() { remove_file(&temp_file_path).expect("Cannot delete temporary file (permission error)"); }
            // The following zip command syntax can only be used on Linux, for Windows a check with cfg!(target_os = "windows")
            // and an equivalent CMD / Powershell command is necessary
            let mut zip_command = Command::new("zip");
            zip_command.arg("-q")
                       .arg("-r")
                       .arg(temp_file_path.to_str().expect("Invalid path encoding (expected UTF-8)"))
                       .arg(directory_name);
            if let Some(parent_path) = STORAGE.join(&path).parent() {
                zip_command.current_dir(parent_path.to_str().expect("Invalid path encoding (expected UTF-8)"));
            }
            zip_command.status().expect("Cannot execute zip command");
            Either::Left(NamedFile::open(&temp_file_path).await.ok())
        }
        else { Either::Right(RawHtml(NO_DIRECTORY.to_owned())) }
    }
    else { Either::Right(RawHtml(ACCESS_DENIED.to_owned())) }
}

#[get("/delete_dir/<path..>")]
fn delete_dir(cookies: &CookieJar<'_>, path: DotPathBuf) -> Either<Redirect, RawHtml<String>> {
    let path = path.0;
    if let Some(username) = check_login(cookies, &path) {
        if check_path(&path).1 {
            let parent_path = path.parent().expect("Cannot extract parent path");
            if parent_path == Path::new("") { return Either::Left(Redirect::to(uri!(list_directory(&username)))) }
            remove_dir_all(STORAGE.join(&path)).expect("Cannot delete directory (permission error)");
            Either::Left(Redirect::to(uri!(list_directory(parent_path.to_str().expect("Invalid path encoding (expected UTF-8)")))))
        }
        else { Either::Right(RawHtml(NO_DIRECTORY.to_owned())) }
    }
    else { Either::Right(RawHtml(ACCESS_DENIED.to_owned())) }
}

#[get("/delete_file/<path..>")]
fn delete_file(cookies: &CookieJar<'_>, path: DotPathBuf) -> Either<Redirect, RawHtml<String>> {
    let path = path.0;
    if let Some(username) = check_login(cookies, &path) {
        if check_path(&path).0 {
            let parent_path = path.parent().expect("Cannot extract parent path");
            if parent_path == Path::new("") { return Either::Left(Redirect::to(uri!(list_directory(&username)))) }
            remove_file(STORAGE.join(&path)).expect("Cannot delete file (permission error)");
            Either::Left(Redirect::to(uri!(list_directory(parent_path.to_str().expect("Invalid path encoding (expected UTF-8)")))))
        }
        else { Either::Right(RawHtml(NO_FILE.to_owned())) }
    }
    else { Either::Right(RawHtml(ACCESS_DENIED.to_owned())) }
}

#[post("/new_dir/<path..>", data = "<data>")]
fn create_directory(cookies: &CookieJar<'_>, path: DotPathBuf, data: Option<Form<FolderName>>) -> Either<Redirect, RawHtml<String>> {
    let path = path.0;
    if let Some(username) = check_login(cookies, &path) {
        if check_path(&path).1 {
            match data {
                None => Either::Left(Redirect::to(uri!(list_directory(&username)))),
                Some(content) => {
                    // Remove some unwanted characters from the directory name (custom selection)
                    let mut new_dir = sanitize_string(&content.folder_name);
                    if new_dir.len() == 0 { new_dir = "new_directory".to_owned(); }
                    let new_path = STORAGE.join(&path).join(&new_dir);
                    if !new_path.try_exists().expect("Cannot access files metadata (permission error)") {
                        create_dir(new_path).expect("Cannot create directory (permission error)");
                        Either::Left(Redirect::to(uri!(list_directory(path.to_str().expect("Invalid path encoding (expected UTF-8)")))))
                    }
                    else { Either::Right(RawHtml(IS_DIRECTORY.to_owned())) }
                }
            }
        }
        else { Either::Right(RawHtml(NO_DIRECTORY.to_owned())) }
    }
    else { Either::Right(RawHtml(ACCESS_DENIED.to_owned())) }
}

#[post("/unpack/<path..>", data = "<data>")]
fn unpack_archive(cookies: &CookieJar<'_>, path: DotPathBuf, data: Option<Form<ArchiveName>>) -> Either<Redirect, RawHtml<String>> {
    let path = path.0;
    if let Some(username) = check_login(cookies, &path) {
        if check_path(&path).1 {
            match data {
                None => Either::Left(Redirect::to(uri!(list_directory(&username)))),
                Some(content) => {
                    // Remove some unwanted characters from the file name (custom selection)
                    let new_dir = sanitize_string(&content.archive_name);
                    if new_dir.len() < 4 || new_dir[new_dir.len() - 4..].to_lowercase() != ".zip" {
                        return Either::Right(RawHtml(NO_FILE.to_owned()))
                    }
                    let source_file = STORAGE.join(&path).join(&new_dir);
                    let target_path = source_file.with_extension("");
                    if !source_file.is_file() {
                        Either::Right(RawHtml(NO_FILE.to_owned()))
                    } else if target_path.try_exists().expect("Cannot access files metadata (permission error)") {
                        Either::Right(RawHtml(IS_DIRECTORY.to_owned()))
                    } else {
                        // The following unzip command syntax can only be used on Linux, for Windows a check
                        // with cfg!(target_os = "windows") and an equivalent CMD / Powershell command is necessary
                        let mut unzip_command = Command::new("unzip");
                        unzip_command.arg("-q")
                                     .arg(source_file.to_str().expect("Invalid path encoding (expected UTF-8)"))
                                     .arg("-d")
                                     .arg(target_path.to_str().expect("Invalid path encoding (expected UTF-8)"));
                        match unzip_command.status() {
                            Err(_) => Either::Right(RawHtml(UNPACK_ERROR.to_owned())),
                            Ok(_) => Either::Left(Redirect::to(uri!(list_directory(path.to_str().expect("Invalid path encoding (expected UTF-8)")))))
                        }
                    }
                }
            }
        }
        else { Either::Right(RawHtml(NO_DIRECTORY.to_owned())) }
    }
    else { Either::Right(RawHtml(ACCESS_DENIED.to_owned())) }
}

#[post("/upload/<path..>", format = "multipart/form-data", data = "<data>")]
async fn upload_file(cookies: &CookieJar<'_>, path: DotPathBuf, mut data: Form<Upload<'_>>) -> Either<Redirect, RawHtml<String>> {
    let path = path.0;
    if let Some(_username) = check_login(cookies, &path) {
        if check_path(&path).1 {
            // Remove some unwanted characters from the file name (custom selection,
            // automatic sanitation would remove dots and the file extension as well)
            let mut file_name = match data.file.raw_name() {
                None => return Either::Right(RawHtml(UPLOAD_ERROR.to_owned())),
                Some(raw_name) => {
                    sanitize_string(raw_name.dangerous_unsafe_unsanitized_raw().as_str())
                }
            };
            while file_name.starts_with(" ") {
                file_name = file_name[1..].to_owned();
            }
            while file_name.ends_with(" ") {
                file_name = file_name[..file_name.len() - 1].to_owned();
            }
            if file_name.len() == 0 {
                return Either::Right(RawHtml(UPLOAD_ERROR.to_owned()))
            }
            else if STORAGE.join(&path).join(&file_name).try_exists().expect("Cannot access files metadata (permission error)") {
                return Either::Right(RawHtml(IS_FILE.to_owned()))
            }
            else {
                // Try persisting the file to the given path
                match data.file.persist_to(STORAGE.join(&path).join(&file_name)).await {
                    Ok(_) => Either::Left(Redirect::to(uri!(list_directory(path.to_str().expect("Invalid path encoding (expected UTF-8)"))))),
                    // If this failed, try to copy the temporary file to the given path
                    // (e.g. the temp path is on a different logical device - see persist_to() docs)
                    Err(_) => match data.file.move_copy_to(STORAGE.join(&path).join(&file_name)).await {
                        Ok(_) => Either::Left(Redirect::to(uri!(list_directory(path.to_str().expect("Invalid path encoding (expected UTF-8)"))))),
                        Err(_) => Either::Right(RawHtml(UPLOAD_ERROR.to_owned()))
                    }
                }
            }
        }
        else { Either::Right(RawHtml(NO_DIRECTORY.to_owned())) }
    }
    else { Either::Right(RawHtml(ACCESS_DENIED.to_owned())) }
}

#[launch]
fn rocket() -> Rocket<Build> {
    // Start an additional thread to clean the tmp directory once in a while
    let tmp_path = STORAGE.join("tmp");
    thread::spawn(move || {
        loop {
            for item in tmp_path.read_dir().expect("Cannot read tmp directory contents") {
                if let Ok(item) = item {
                    if item.path().is_file() { remove_file(item.path()).expect("Cannot delete temporary file (permission error)"); }
                }
            }
            thread::sleep(Duration::from_secs(CONFIG.clean_tmp_files));
        }
    });
    // Launch the server
    rocket::build()
        .mount("/", routes![home, login, list_directory, favicon, download_file, download_folder, delete_dir, delete_file, create_directory, unpack_archive, upload_file])
        .mount("/icons", FileServer::from("icons"))
}
