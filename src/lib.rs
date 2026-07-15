//! LoopWire : plugin OBS (Rust, cdylib) pour le mute/volume/mapping-démapping
//! d'une carte de capture audio USB vers PipeWire/PulseAudio, exposé via des
//! requêtes vendor obs-websocket, pour que le serveur MCP obs-mcp puisse les
//! appeler directement.
//!
//! Le pattern d'intégration vendor (obs_get_proc_handler/proc_handler_call/
//! calldata_*) est porté à partir du vrai header officiel obs-websocket-api.h
//! (documentation/references/upstream/plugins/sources/source-record/ du
//! projet obs-mcp) — pas inventé. Ce header est constitué de fonctions C
//! `static inline`, donc sans symbole à lier directement depuis Rust ; leur
//! logique est réimplémentée ici en appelant les deux vraies fonctions
//! exportées par libobs dont elles dépendent (`calldata_get_data`/
//! `calldata_set_data`), avec la sécurité mémoire de Rust pour tout le reste
//! (pas de `system()`/manipulation de buffers C bruts comme dans une version
//! C équivalente : `std::process::Command` sans shell, JSON via `serde_json`).

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

mod bindings {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

use bindings::{
    blog, calldata, calldata_get_data, calldata_set_data, obs_data_get_bool, obs_data_get_int,
    obs_data_get_string, obs_data_set_bool, obs_data_set_int, obs_data_set_string, obs_data_t,
    obs_frontend_add_dock_by_id, obs_get_proc_handler, obs_module_t, proc_handler_call,
    proc_handler_t,
};

use serde::{Deserialize, Serialize};
use std::ffi::{c_char, c_void, CStr, CString};
use std::mem;
use std::process::Command;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Mutex;

const LOG_INFO: i32 = 200;
const LOG_WARNING: i32 = 300;

fn log_line(level: i32, msg: &str) {
    // blog() est variadique côté C ; on se limite volontairement au format
    // fixe "%s" + une seule chaîne, seule forme d'appel sûre depuis Rust
    // sans utiliser l'API variadique instable.
    if let (Ok(fmt), Ok(cmsg)) = (CString::new("%s"), CString::new(msg)) {
        unsafe { blog(level, fmt.as_ptr(), cmsg.as_ptr()) };
    }
}

// calldata_t : helpers sûrs autour de calldata_get_data/calldata_set_data
// (les deux seules fonctions réellement exportées par libobs ; toutes les
// autres fonctions calldata_get/calldata_set du header C sont "static
// inline" et n'existent pas comme symboles à lier).

fn calldata_new() -> calldata {
    // Layout garanti correct par bindgen (généré depuis le vrai calldata.h),
    // zero-init exactement équivalent au `calldata_t cd = {0, 0, 0, 0};` du C.
    unsafe { mem::zeroed() }
}

fn cd_set_string(cd: &mut calldata, name: &str, value: &str) -> Option<()> {
    let cname = CString::new(name).ok()?;
    let cvalue = CString::new(value).ok()?;
    unsafe {
        calldata_set_data(
            cd,
            cname.as_ptr(),
            cvalue.as_ptr() as *const c_void,
            cvalue.as_bytes_with_nul().len(),
        );
    }
    Some(())
}

fn cd_set_ptr(cd: &mut calldata, name: &str, ptr: *mut c_void) -> Option<()> {
    let cname = CString::new(name).ok()?;
    unsafe {
        calldata_set_data(
            cd,
            cname.as_ptr(),
            &ptr as *const _ as *const c_void,
            mem::size_of::<*mut c_void>(),
        );
    }
    Some(())
}

fn cd_get_ptr(cd: &calldata, name: &str) -> Option<*mut c_void> {
    let cname = CString::new(name).ok()?;
    let mut out: *mut c_void = std::ptr::null_mut();
    let ok = unsafe {
        calldata_get_data(
            cd,
            cname.as_ptr(),
            &mut out as *mut _ as *mut c_void,
            mem::size_of::<*mut c_void>(),
        )
    };
    if ok {
        Some(out)
    } else {
        None
    }
}

fn cd_get_bool(cd: &calldata, name: &str) -> bool {
    let Ok(cname) = CString::new(name) else {
        return false;
    };
    let mut out: bool = false;
    unsafe {
        calldata_get_data(
            cd,
            cname.as_ptr(),
            &mut out as *mut _ as *mut c_void,
            mem::size_of::<bool>(),
        )
    };
    out
}

/* ------------------------------------------------------------------- */
/* Port de la partie "VENDOR API" d'obs-websocket-api.h                 */
/* ------------------------------------------------------------------- */

type RequestCallback =
    unsafe extern "C" fn(*mut obs_data_t, *mut obs_data_t, *mut c_void);

#[repr(C)]
struct ObsWebsocketRequestCallback {
    callback: RequestCallback,
    priv_data: *mut c_void,
}

static PROC_HANDLER: AtomicPtr<proc_handler_t> = AtomicPtr::new(std::ptr::null_mut());

fn ensure_ph() -> Option<*mut proc_handler_t> {
    let existing = PROC_HANDLER.load(Ordering::Acquire);
    if !existing.is_null() {
        return Some(existing);
    }

    let global_ph = unsafe { obs_get_proc_handler() };
    if global_ph.is_null() {
        return None;
    }

    let mut cd = calldata_new();
    let name = CString::new("obs_websocket_api_get_ph").ok()?;
    unsafe { proc_handler_call(global_ph, name.as_ptr(), &mut cd) };
    let ph = cd_get_ptr(&cd, "ph")? as *mut proc_handler_t;
    if ph.is_null() {
        return None;
    }
    PROC_HANDLER.store(ph, Ordering::Release);
    Some(ph)
}

fn vendor_register(name: &str) -> Option<*mut c_void> {
    let ph = ensure_ph()?;
    let mut cd = calldata_new();
    cd_set_string(&mut cd, "name", name)?;
    let proc_name = CString::new("vendor_register").ok()?;
    unsafe { proc_handler_call(ph, proc_name.as_ptr(), &mut cd) };
    cd_get_ptr(&cd, "vendor")
}

fn vendor_register_request(vendor: *mut c_void, request_type: &str, callback: RequestCallback) -> bool {
    let Some(ph) = ensure_ph() else { return false };

    let cb = Box::new(ObsWebsocketRequestCallback {
        callback,
        priv_data: std::ptr::null_mut(),
    });
    // Volontairement fuité : obs-websocket garde cette structure en mémoire
    // tant que la requête vendor reste enregistrée, soit toute la durée de
    // vie du plugin (comme le fait la version C avec un `static` local).
    let cb_ptr = Box::into_raw(cb) as *mut c_void;

    let mut cd = calldata_new();
    let Some(_) = cd_set_string(&mut cd, "type", request_type) else {
        return false;
    };
    let Some(_) = cd_set_ptr(&mut cd, "callback", cb_ptr) else {
        return false;
    };
    let Some(_) = cd_set_ptr(&mut cd, "vendor", vendor) else {
        return false;
    };
    let Ok(proc_name) = CString::new("vendor_request_register") else {
        return false;
    };
    unsafe { proc_handler_call(ph, proc_name.as_ptr(), &mut cd) };
    cd_get_bool(&cd, "success")
}

/* ------------------------------------------------------------------- */
/* obs_data_t : lecture/écriture des arguments de requête (sûr, pas de   */
/* buffer C manuel — juste des conversions CStr/CString aux frontières)  */
/* ------------------------------------------------------------------- */

fn data_get_string(data: *mut obs_data_t, key: &str) -> Option<String> {
    let ckey = CString::new(key).ok()?;
    let ptr = unsafe { obs_data_get_string(data, ckey.as_ptr()) };
    if ptr.is_null() {
        return None;
    }
    let s = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn data_get_bool(data: *mut obs_data_t, key: &str) -> bool {
    let Ok(ckey) = CString::new(key) else {
        return false;
    };
    unsafe { obs_data_get_bool(data, ckey.as_ptr()) }
}

fn data_get_int(data: *mut obs_data_t, key: &str) -> i64 {
    let Ok(ckey) = CString::new(key) else {
        return 0;
    };
    unsafe { obs_data_get_int(data, ckey.as_ptr()) }
}

fn data_set_string(data: *mut obs_data_t, key: &str, value: &str) {
    if let (Ok(ckey), Ok(cvalue)) = (CString::new(key), CString::new(value)) {
        unsafe { obs_data_set_string(data, ckey.as_ptr(), cvalue.as_ptr()) };
    }
}

fn data_set_bool(data: *mut obs_data_t, key: &str, value: bool) {
    if let Ok(ckey) = CString::new(key) {
        unsafe { obs_data_set_bool(data, ckey.as_ptr(), value) };
    }
}

fn data_set_int(data: *mut obs_data_t, key: &str, value: i64) {
    if let Ok(ckey) = CString::new(key) {
        unsafe { obs_data_set_int(data, ckey.as_ptr(), value) };
    }
}

/* ------------------------------------------------------------------- */
/* Config persistée (~/.config/loopwire/config.json)                    */
/* ------------------------------------------------------------------- */

// Aucune valeur de carte/source par défaut : chaque machine a un matériel de
// capture différent (ou aucun) — figer un nom précis (l'ancien
// "alsa_card.usb-MACROSILICON_USB_Video-02" par exemple) n'aurait de sens que
// sur la machine où ç'a été écrit. Vide tant que l'utilisateur n'a pas
// explicitement choisi dans la Configuration, parmi la vraie liste détectée
// sur SA machine.
#[derive(Serialize, Deserialize, Clone)]
struct PluginConfig {
    card: String,
    source: String,
    /// Nom de sink utilisé seulement si `sink_auto` est false.
    sink: String,
    /// Si vrai (par défaut), la sortie réelle utilisée est celle que le
    /// système considère comme "par défaut" au moment de mapper (`pactl
    /// get-default-sink`), interrogée une seule fois à cet instant — pas de
    /// sondage en continu. Ça a du sens comme défaut, contrairement à la
    /// carte/source : il existe un vrai concept de "sortie par défaut"
    /// fiable côté système, valable pour n'importe qui.
    #[serde(default = "default_true")]
    sink_auto: bool,
}

fn default_true() -> bool {
    true
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            card: String::new(),
            source: String::new(),
            sink: String::new(),
            sink_auto: true,
        }
    }
}

/// Sortie par défaut du système, interrogée une seule fois au moment de
/// l'appel (jamais en tâche de fond).
fn default_sink() -> Option<String> {
    let out = pactl(&["get-default-sink"]);
    let name = out.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Résout la sortie réellement utilisée pour cet appel : automatique
/// (système) si `sink_auto`, sinon la valeur manuelle enregistrée. Utilisé
/// partout où le code agissait sur `cfg.sink` directement.
fn effective_sink(cfg: &PluginConfig) -> String {
    if cfg.sink_auto {
        default_sink().unwrap_or_else(|| cfg.sink.clone())
    } else {
        cfg.sink.clone()
    }
}

fn config_path() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(std::path::Path::new(&home).join(".config/loopwire/config.json"))
}

fn load_config() -> PluginConfig {
    (|| {
        let path = config_path()?;
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    })()
    .unwrap_or_default()
}

fn save_config(cfg: &PluginConfig) {
    let Some(path) = config_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(path, text);
    }
}

static CONFIG: Mutex<Option<PluginConfig>> = Mutex::new(None);

fn with_config<R>(f: impl FnOnce(&mut PluginConfig) -> R) -> R {
    let mut guard = CONFIG.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(load_config());
    }
    f(guard.as_mut().unwrap())
}

/// Copie la config sous verrou bref, PUIS relâche le verrou avant de faire le
/// moindre appel `pactl` (qui peut prendre de quelques dizaines de ms à
/// plusieurs secondes). Sans ça, le thread d'arrière-plan (rafraîchissement
/// toutes les 2s, plusieurs appels `pactl` séquentiels) et une action
/// utilisateur (Map/Unmap/Mute) se bloquent mutuellement sur le même verrou
/// pendant toute la durée de leurs appels `pactl` respectifs — cause exacte
/// de la latence de ~2s observée sur Map/Unmap.
fn snapshot_config() -> PluginConfig {
    with_config(|cfg| cfg.clone())
}

/* ------------------------------------------------------------------- */
/* pactl (std::process::Command — pas de shell, pas d'injection possible) */
/* ------------------------------------------------------------------- */

fn pactl(args: &[&str]) -> String {
    Command::new("pactl")
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn source_exists(name: &str) -> bool {
    !name.is_empty() && pactl(&["list", "sources", "short"]).contains(name)
}

fn get_mute(name: &str) -> Option<bool> {
    let out = pactl(&["get-source-mute", name]);
    if out.contains("Mute: yes") {
        Some(true)
    } else if out.contains("Mute: no") {
        Some(false)
    } else {
        None
    }
}

fn get_volume_percent(name: &str) -> Option<i32> {
    let out = pactl(&["get-source-volume", name]);
    out.split_whitespace()
        .find_map(|tok| tok.strip_suffix('%').and_then(|n| n.parse::<i32>().ok()))
}

fn set_mute(name: &str, muted: bool) {
    pactl(&["set-source-mute", name, if muted { "1" } else { "0" }]);
}

fn set_volume_percent(name: &str, percent: i32) {
    let value = format!("{percent}%");
    pactl(&["set-source-volume", name, &value]);
}

fn find_loopback_module_ids(source: &str, sink: &str) -> Vec<String> {
    // Une source/sink vide correspondrait trivialement (sous-chaîne vide) à
    // n'importe quel module déjà chargé — jamais "mappé" tant que non configuré.
    if source.is_empty() || sink.is_empty() {
        return Vec::new();
    }
    pactl(&["list", "modules", "short"])
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let id = parts.next()?;
            let kind = parts.next()?;
            let args = parts.next()?;
            if kind == "module-loopback" && args.contains(source) && args.contains(sink) {
                Some(id.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn is_mapped(source: &str, sink: &str) -> bool {
    !find_loopback_module_ids(source, sink).is_empty()
}

fn wait_for_source(source: &str) {
    for _ in 0..10 {
        if source_exists(source) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

/// Bascule le profil de la carte (off puis on) UNIQUEMENT si la source
/// n'existe pas déjà — cette bascule force PipeWire à recréer le node, mais
/// c'est une opération perturbatrice pour ce type de périphérique composite
/// (audio+vidéo dans le même boîtier USB) : un OBS réel a planté (SIGABRT
/// dans linux-alsa.so) après cette même bascule pendant qu'une capture était
/// active dessus, et une corruption du décodage MJPEG a été observée plus
/// tard dans la même session. On ne la déclenche donc que quand c'est
/// réellement nécessaire, jamais "pour être sûr".
fn do_map(card: &str, source: &str, sink: &str) -> (bool, String) {
    if !source_exists(source) {
        pactl(&["set-card-profile", card, "off"]);
        pactl(&["set-card-profile", card, "input:analog-stereo"]);
        wait_for_source(source);
    }

    if !source_exists(source) {
        return (
            false,
            "Failed: the PipeWire node did not appear (device probably held by another program).".into(),
        );
    }

    if find_loopback_module_ids(source, sink).is_empty() {
        let source_arg = format!("source={source}");
        let sink_arg = format!("sink={sink}");
        pactl(&["load-module", "module-loopback", &source_arg, &sink_arg]);
    }

    set_mute(source, false);
    (true, "Mapped.".into())
}

fn do_unmap(source: &str, sink: &str) -> String {
    let ids = find_loopback_module_ids(source, sink);
    if ids.is_empty() {
        return "Nothing to unmap (no loopback loaded).".into();
    }
    for id in &ids {
        pactl(&["unload-module", id]);
    }
    format!("Unmapped ({} module(s) unloaded).", ids.len())
}

/* ------------------------------------------------------------------- */
/* Handlers vendor obs-websocket                                        */
/* ------------------------------------------------------------------- */

unsafe extern "C" fn websocket_get_status(
    _request_data: *mut obs_data_t,
    response_data: *mut obs_data_t,
    _param: *mut c_void,
) {
    let cfg = snapshot_config();
    let exists = source_exists(&cfg.source);
    data_set_bool(response_data, "source_exists", exists);
    data_set_bool(response_data, "mapped", is_mapped(&cfg.source, &effective_sink(&cfg)));
    if exists {
        if let Some(muted) = get_mute(&cfg.source) {
            data_set_bool(response_data, "muted", muted);
        }
        if let Some(percent) = get_volume_percent(&cfg.source) {
            data_set_int(response_data, "volume_percent", percent as i64);
        }
    }
    data_set_string(response_data, "card", &cfg.card);
    data_set_string(response_data, "source", &cfg.source);
    data_set_string(response_data, "sink", &cfg.sink);
    data_set_bool(response_data, "sink_auto", cfg.sink_auto);
    data_set_bool(response_data, "success", true);
}

unsafe extern "C" fn websocket_set_mute(
    request_data: *mut obs_data_t,
    response_data: *mut obs_data_t,
    _param: *mut c_void,
) {
    let muted = data_get_bool(request_data, "muted");
    let cfg = snapshot_config();
    set_mute(&cfg.source, muted);
    data_set_bool(response_data, "success", true);
}

unsafe extern "C" fn websocket_set_volume(
    request_data: *mut obs_data_t,
    response_data: *mut obs_data_t,
    _param: *mut c_void,
) {
    let percent = data_get_int(request_data, "volume_percent");
    if !(0..=150).contains(&percent) {
        data_set_string(response_data, "error", "volume_percent must be between 0 and 150");
        data_set_bool(response_data, "success", false);
        return;
    }
    let cfg = snapshot_config();
    set_volume_percent(&cfg.source, percent as i32);
    data_set_bool(response_data, "success", true);
}

unsafe extern "C" fn websocket_map(
    _request_data: *mut obs_data_t,
    response_data: *mut obs_data_t,
    _param: *mut c_void,
) {
    let cfg = snapshot_config();
    let sink = effective_sink(&cfg);
    let (ok, status) = do_map(&cfg.card, &cfg.source, &sink);
    data_set_string(response_data, "status", &status);
    data_set_bool(response_data, "success", ok);
}

unsafe extern "C" fn websocket_unmap(
    _request_data: *mut obs_data_t,
    response_data: *mut obs_data_t,
    _param: *mut c_void,
) {
    let cfg = snapshot_config();
    let sink = effective_sink(&cfg);
    let status = do_unmap(&cfg.source, &sink);
    data_set_string(response_data, "status", &status);
    data_set_bool(response_data, "success", true);
}

unsafe extern "C" fn websocket_get_config(
    _request_data: *mut obs_data_t,
    response_data: *mut obs_data_t,
    _param: *mut c_void,
) {
    with_config(|cfg| {
        data_set_string(response_data, "card", &cfg.card);
        data_set_string(response_data, "source", &cfg.source);
        data_set_string(response_data, "sink", &cfg.sink);
        data_set_bool(response_data, "sink_auto", cfg.sink_auto);
    });
    data_set_bool(response_data, "success", true);
}

unsafe extern "C" fn websocket_set_config(
    request_data: *mut obs_data_t,
    response_data: *mut obs_data_t,
    _param: *mut c_void,
) {
    let card = data_get_string(request_data, "card");
    let source = data_get_string(request_data, "source");
    let sink = data_get_string(request_data, "sink");
    let sink_auto = data_get_bool(request_data, "sink_auto");

    with_config(|cfg| {
        if let Some(v) = card {
            cfg.card = v;
        }
        if let Some(v) = source {
            cfg.source = v;
        }
        if let Some(v) = sink {
            cfg.sink = v;
        }
        cfg.sink_auto = sink_auto;
        save_config(cfg);
    });
    data_set_bool(response_data, "success", true);
}

// FFI appelée par le shim C++ (src/dock.cpp) pour le dock OBS natif — même
// logique que les handlers vendor ci-dessus, exposée avec des types C plats
// pour le pont Qt. Les chaînes renvoyées sont allouées par CString::into_raw
// et DOIVENT être libérées côté C++ via loopwire_free_string (jamais
// via `free()`/`delete` directement : l'allocateur est celui de Rust).

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FfiStatus {
    source_exists: bool,
    mapped: bool,
    muted: bool,
    volume_percent: i32,
}

impl Default for FfiStatus {
    fn default() -> Self {
        FfiStatus { source_exists: false, mapped: false, muted: false, volume_percent: 0 }
    }
}

fn compute_status() -> FfiStatus {
    let cfg = snapshot_config();
    let exists = source_exists(&cfg.source);
    let muted = if exists { get_mute(&cfg.source).unwrap_or(false) } else { false };
    let volume_percent = if exists { get_volume_percent(&cfg.source).unwrap_or(0) } else { 0 };
    FfiStatus {
        source_exists: exists,
        mapped: is_mapped(&cfg.source, &effective_sink(&cfg)),
        muted,
        volume_percent,
    }
}

static CACHED_STATUS: Mutex<Option<FfiStatus>> = Mutex::new(None);
static BACKGROUND_REFRESH_STARTED: std::sync::Once = std::sync::Once::new();

/// Le rafraîchissement périodique (dock Qt, toutes les 2s) ne doit JAMAIS
/// bloquer le thread principal d'OBS avec des appels `pactl` synchrones —
/// un `abort()` réel dans le plugin ALSA natif d'OBS (linux-alsa.so) a été
/// observé un jour de dépannage intensif de ce même périphérique composite,
/// et une corruption du décodage vidéo MJPEG plus tard dans la même session
/// a disparu dès que ce genre d'activité périodique a cessé. Ce thread fait
/// tout le travail bloquant en arrière-plan ; le thread principal ne lit
/// jamais qu'un résultat déjà prêt.
fn ensure_background_refresh_started() {
    BACKGROUND_REFRESH_STARTED.call_once(|| {
        std::thread::spawn(|| loop {
            let status = compute_status();
            *CACHED_STATUS.lock().unwrap_or_else(|e| e.into_inner()) = Some(status);
            std::thread::sleep(std::time::Duration::from_millis(2000));
        });
    });
}

fn string_to_raw(s: String) -> *mut c_char {
    CString::new(s).map(CString::into_raw).unwrap_or(std::ptr::null_mut())
}

unsafe fn str_from_c(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    Some(CStr::from_ptr(ptr).to_string_lossy().into_owned())
}

#[no_mangle]
pub extern "C" fn loopwire_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            drop(CString::from_raw(s));
        }
    }
}

#[no_mangle]
pub extern "C" fn loopwire_get_status() -> FfiStatus {
    ensure_background_refresh_started();
    // Premier appel : le thread d'arrière-plan n'a peut-être pas encore
    // produit son premier résultat — dans ce cas seulement, on calcule une
    // fois en direct (un unique appel bloquant au tout premier affichage du
    // dock n'est pas le problème ; c'est la répétition toutes les 2s qui
    // l'était).
    let cached = *CACHED_STATUS.lock().unwrap_or_else(|e| e.into_inner());
    cached.unwrap_or_else(compute_status)
}

#[no_mangle]
pub extern "C" fn loopwire_set_mute(muted: bool) {
    let cfg = snapshot_config();
    set_mute(&cfg.source, muted);
}

#[no_mangle]
pub extern "C" fn loopwire_set_volume(percent: i32) {
    let percent = percent.clamp(0, 150);
    let cfg = snapshot_config();
    set_volume_percent(&cfg.source, percent);
}

#[no_mangle]
pub extern "C" fn loopwire_do_map() -> *mut c_char {
    let cfg = snapshot_config();
    let sink = effective_sink(&cfg);
    let (_, status) = do_map(&cfg.card, &cfg.source, &sink);
    string_to_raw(status)
}

#[no_mangle]
pub extern "C" fn loopwire_do_unmap() -> *mut c_char {
    let cfg = snapshot_config();
    let sink = effective_sink(&cfg);
    let status = do_unmap(&cfg.source, &sink);
    string_to_raw(status)
}

#[no_mangle]
pub extern "C" fn loopwire_get_config_card() -> *mut c_char {
    with_config(|cfg| string_to_raw(cfg.card.clone()))
}

#[no_mangle]
pub extern "C" fn loopwire_get_config_source() -> *mut c_char {
    with_config(|cfg| string_to_raw(cfg.source.clone()))
}

#[no_mangle]
pub extern "C" fn loopwire_get_config_sink() -> *mut c_char {
    with_config(|cfg| string_to_raw(cfg.sink.clone()))
}

#[no_mangle]
pub extern "C" fn loopwire_get_config_sink_auto() -> bool {
    with_config(|cfg| cfg.sink_auto)
}

/// Sortie par défaut du système actuelle (`pactl get-default-sink`, un seul
/// appel ponctuel) — utilisé par le dialogue pour afficher ce que le mode
/// automatique résoudrait concrètement, sans avoir à deviner.
#[no_mangle]
pub extern "C" fn loopwire_get_default_sink() -> *mut c_char {
    string_to_raw(default_sink().unwrap_or_default())
}

#[no_mangle]
pub extern "C" fn loopwire_set_config(
    card: *const c_char,
    source: *const c_char,
    sink: *const c_char,
    sink_auto: bool,
) {
    unsafe {
        let card = str_from_c(card);
        let source = str_from_c(source);
        let sink = str_from_c(sink);
        with_config(|cfg| {
            if let Some(v) = card {
                if !v.is_empty() {
                    cfg.card = v;
                }
            }
            if let Some(v) = source {
                if !v.is_empty() {
                    cfg.source = v;
                }
            }
            if let Some(v) = sink {
                if !v.is_empty() {
                    cfg.sink = v;
                }
            }
            cfg.sink_auto = sink_auto;
            save_config(cfg);
        });
    }
}

fn pactl_list_names(kind: &str) -> String {
    pactl(&["list", kind, "short"])
        .lines()
        .filter_map(|line| line.split('\t').nth(1))
        .collect::<Vec<_>>()
        .join("\n")
}

#[no_mangle]
pub extern "C" fn loopwire_list_cards() -> *mut c_char {
    string_to_raw(pactl_list_names("cards"))
}

#[no_mangle]
pub extern "C" fn loopwire_list_sources() -> *mut c_char {
    string_to_raw(pactl_list_names("sources"))
}

#[no_mangle]
pub extern "C" fn loopwire_list_sinks() -> *mut c_char {
    string_to_raw(pactl_list_names("sinks"))
}

extern "C" {
    fn loopwire_create_dock_widget() -> *mut c_void;
}

/* ------------------------------------------------------------------- */
/* Cycle de vie du module OBS                                           */
/* ------------------------------------------------------------------- */

static MODULE_POINTER: AtomicPtr<obs_module_t> = AtomicPtr::new(std::ptr::null_mut());

#[no_mangle]
pub extern "C" fn obs_module_set_pointer(module: *mut obs_module_t) {
    MODULE_POINTER.store(module, Ordering::Release);
}

#[no_mangle]
pub extern "C" fn obs_module_ver() -> u32 {
    (bindings::LIBOBS_API_MAJOR_VER << 24) | (bindings::LIBOBS_API_MINOR_VER << 16) | bindings::LIBOBS_API_PATCH_VER
}

#[no_mangle]
pub extern "C" fn obs_module_load() -> bool {
    // Charge la config une fois ici plutôt que paresseusement, pour que les
    // erreurs de fichier éventuelles apparaissent dans les logs OBS au
    // démarrage plutôt qu'au premier appel vendor.
    with_config(|_| {});

    if let (Ok(id), Ok(title)) = (CString::new("loopwire-dock"), CString::new("LoopWire")) {
        unsafe {
            let widget = loopwire_create_dock_widget();
            if !obs_frontend_add_dock_by_id(id.as_ptr(), title.as_ptr(), widget) {
                log_line(LOG_WARNING, "[loopwire] failed to register the dock.");
            }
        }
    }

    let Some(vendor) = vendor_register("loopwire") else {
        log_line(LOG_WARNING, "[loopwire] obs-websocket not found — plugin loaded without vendor requests.");
        return true;
    };

    vendor_register_request(vendor, "get_status", websocket_get_status);
    vendor_register_request(vendor, "set_mute", websocket_set_mute);
    vendor_register_request(vendor, "set_volume", websocket_set_volume);
    vendor_register_request(vendor, "map", websocket_map);
    vendor_register_request(vendor, "unmap", websocket_unmap);
    vendor_register_request(vendor, "get_config", websocket_get_config);
    vendor_register_request(vendor, "set_config", websocket_set_config);

    log_line(LOG_INFO, "[loopwire] plugin loaded, obs-websocket vendor registered.");
    true
}

#[no_mangle]
pub extern "C" fn obs_module_unload() {
    // Démappage automatique sur fermeture propre d'OBS uniquement : le
    // loopback n'a de sens que le temps où OBS l'utilise. Note : cette
    // fonction n'est PAS appelée en cas de crash/kill -9/coupure de courant —
    // seulement lors d'une fermeture normale.
    let cfg = snapshot_config();
    let sink = effective_sink(&cfg);
    do_unmap(&cfg.source, &sink);
    log_line(LOG_INFO, "[loopwire] plugin unloaded, loopback unmapped.");
}

#[no_mangle]
pub extern "C" fn obs_module_name() -> *const c_char {
    c"LoopWire".as_ptr()
}

#[no_mangle]
pub extern "C" fn obs_module_description() -> *const c_char {
    c"Mute/volume/mapping of a USB audio capture card into PipeWire, exposed via obs-websocket vendor requests.".as_ptr()
}
