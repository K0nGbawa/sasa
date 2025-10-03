/// Simple And Stupid Audio for Rust, optimized for low latency.
pub mod backend;
use atomic_float::AtomicF64;
pub use backend::Backend;

mod clip;
pub use clip::AudioClip;

mod mixer;

mod renderer;
pub use renderer::{Music, MusicParams, PlaySfxParams, Renderer, Sfx};

use crate::{backend::BackendSetup, mixer::MixerCommand};
use anyhow::{anyhow, Context, Result};
use ringbuf::{HeapProducer, HeapRb};
use std::{
    ffi::{c_char, CStr}, ops::{Add, Mul}, slice, sync::{
        atomic::Ordering,
        Arc,
    }
};

fn buffer_is_full<E>(_: E) -> anyhow::Error {
    anyhow!("buffer is full")
}

#[derive(Clone, Copy, Default)]
pub struct Frame(pub f32, pub f32);
impl Frame {
    pub fn avg(&self) -> f32 {
        (self.0 + self.1) / 2.
    }

    pub fn interpolate(&self, other: &Self, f: f32) -> Self {
        Self(
            self.0 + (other.0 - self.0) * f,
            self.1 + (other.1 - self.1) * f,
        )
    }
}
impl Add for Frame {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0, self.1 + rhs.1)
    }
}
impl Mul<f32> for Frame {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self::Output {
        Self(self.0 * rhs, self.1 * rhs)
    }
}

const LATENCY_RECORD_NUM: usize = 640;

pub struct LatencyRecorder {
    records: [f64; LATENCY_RECORD_NUM],
    head: usize,
    sum: f64,
    full: bool,
    result: Arc<AtomicF64>,
}

impl LatencyRecorder {
    pub fn new(result: Arc<AtomicF64>) -> Self {
        Self {
            records: [0.; LATENCY_RECORD_NUM],
            head: 0,
            sum: 0.,
            full: false,
            result,
        }
    }

    pub fn push(&mut self, record: f64) {
        let place = &mut self.records[self.head];
        self.sum += record - *place;
        *place = record;
        self.head += 1;
        if self.head == LATENCY_RECORD_NUM {
            self.full = true;
            self.head = 0;
        }
        self.result.store(
            self.sum
                / (if self.full {
                    LATENCY_RECORD_NUM
                } else {
                    self.head.max(1)
                }) as f64,
            Ordering::SeqCst,
        );
    }
}

pub struct AudioManager {
    backend: Box<dyn Backend>,
    latency: Arc<AtomicF64>,
    prod: HeapProducer<MixerCommand>,
}

impl AudioManager {
    pub fn new(backend: impl Backend + 'static) -> Result<Self> {
        Self::new_box(Box::new(backend))
    }

    pub fn new_box(mut backend: Box<dyn Backend>) -> Result<Self> {
        let (prod, cons) = HeapRb::new(16).split();
        let latency: Arc<AtomicF64> = Arc::default();
        let latency_rec = LatencyRecorder::new(Arc::clone(&latency));
        backend.setup(BackendSetup {
            mixer_cons: cons,
            latency_rec,
        })?;
        backend.start()?;
        Ok(Self {
            backend,
            latency,
            prod,
        })
    }

    pub fn create_sfx(&mut self, clip: AudioClip, buffer_size: Option<usize>) -> Result<Sfx> {
        let (sfx, sfx_renderer) = Sfx::new(clip, buffer_size);
        self.add_renderer(sfx_renderer)?;
        Ok(sfx)
    }

    pub fn create_music(&mut self, clip: AudioClip, settings: MusicParams) -> Result<Music> {
        let (music, music_renderer) = Music::new(clip, settings);
        self.add_renderer(music_renderer)?;
        Ok(music)
    }

    pub fn add_renderer(&mut self, renderer: impl Renderer + 'static) -> Result<()> {
        self.prod
            .push(MixerCommand::AddRenderer(Box::new(renderer)))
            .map_err(buffer_is_full)
            .context("add renderer")?;
        Ok(())
    }

    pub fn estimate_latency(&self) -> f64 {
        self.latency.load(Ordering::SeqCst)
    }

    #[inline(always)]
    pub fn consume_broken(&self) -> bool {
        self.backend.consume_broken()
    }

    #[inline(always)]
    pub fn start(&mut self) -> Result<()> {
        self.backend.start()
    }

    pub fn recover_if_needed(&mut self) -> Result<()> {
        if self.consume_broken() {
            self.start()
        } else {
            Ok(())
        }
    }
}

#[no_mangle]
pub extern "C" fn create_audio_manager() -> *mut AudioManager {
    #[cfg(all(not(feature="cpal"), not(feature="oboe")))]
    return std::ptr::null_mut();
    #[cfg(feature="cpal")]
    {
        let settings = backend::cpal::CpalSettings::default();
        let backend = Box::new(backend::cpal::CpalBackend::new(settings));
        match AudioManager::new_box(backend){
            Ok(manager) => Box::into_raw(Box::new(manager)),
            Err(_) => std::ptr::null_mut(),
        }
    }
    #[cfg(feature="oboe")]
    {
        let settings = backend::oboe::OboeSettings {
            performance_mode: backend::oboe::PerformanceMode::LowLatency,
            ..Default::default()
        };
        let backend = Box::new(backend::oboe::OboeBackend::new(settings));
        match AudioManager::new_box(backend) {
            Ok(manager) => Box::into_raw(Box::new(manager)),
            Err(_) => std::ptr::null_mut(),
        }
    }
}

#[no_mangle]
pub extern "C" fn recover_if_needed(manager_ptr: *mut AudioManager) -> bool {
    if manager_ptr.is_null() {
        return false;
    }
    let manager = unsafe { match manager_ptr.as_mut() {
            Some(manager) => manager,
            None => return false,
        }
    };
    manager.recover_if_needed().is_ok()
}

#[no_mangle]
pub extern "C" fn load_audio_clip(path: *const c_char) -> *mut AudioClip {
    if path.is_null() {
        return std::ptr::null_mut();
    }
    let path = unsafe { CStr::from_ptr(path) };
    match std::fs::read(path.to_str().unwrap()) {
        Ok(data) => {
            match AudioClip::new(data) {
                Ok(clip) => Box::into_raw(Box::new(clip)),
                Err(_) => std::ptr::null_mut(),
            }
        }
        _ => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "C" fn load_audio_clip_from_buffer(data: *const u8, size: usize) -> *mut AudioClip {
    if data.is_null() {
        return std::ptr::null_mut();
    }
    let data = unsafe { slice::from_raw_parts(data, size) };
    match AudioClip::new(data.to_vec()) {
        Ok(clip) => Box::into_raw(Box::new(clip)), 
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "C" fn create_sfx(manager_ptr: *mut AudioManager, clip_ptr: *mut AudioClip) -> *mut Sfx {
    if manager_ptr.is_null() || clip_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let manager = unsafe { manager_ptr.as_mut().unwrap() };
    let clip = unsafe { &*clip_ptr };
    match manager.create_sfx(clip.clone(), Some(1024)) {
        Ok(sfx) => {
            Box::into_raw(Box::new(sfx))
        },
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "C" fn create_music(manager_ptr: *mut AudioManager, clip_ptr: *mut AudioClip, playback_rate: f64) -> *mut Music {
    if manager_ptr.is_null() || clip_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let manager = unsafe { manager_ptr.as_mut().unwrap() };
    let clip = unsafe { &*clip_ptr };
    let params = MusicParams {
        playback_rate,
        ..Default::default()
    };
    match manager.create_music(clip.clone(), params) {
        Ok(music) => {
            Box::into_raw(Box::new(music))
        },
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "C" fn play_sfx(sfx_ptr: *mut Sfx, volume: f32) -> bool {
    if sfx_ptr.is_null() {
        return false;
    }
    let sfx = unsafe { sfx_ptr.as_mut().unwrap() };
    sfx.play(PlaySfxParams { amplifier: volume }).is_ok()
}

#[no_mangle]
pub extern "C" fn play_music(music_ptr: *mut Music, volume: f32) -> bool {
    if music_ptr.is_null() {
        return false;
    }
    let music = unsafe { music_ptr.as_mut().unwrap() };
    match music.set_amplifier(volume) {
        Ok(_) => music.play().is_ok(),
        Err(_) => false,
    }
}

#[no_mangle]
pub extern "C" fn pause_music(music_ptr: *mut Music) -> bool {
    if music_ptr.is_null() {
        return false;
    }
    let music = unsafe { music_ptr.as_mut().unwrap() };
    music.pause().is_ok()
}

#[no_mangle]
pub extern "C" fn is_music_paused(music_ptr: *mut Music) -> bool {
    if music_ptr.is_null() {
        return true;
    }
    let music = unsafe { music_ptr.as_mut().unwrap() };
    music.paused()
}

#[no_mangle]
pub extern "C" fn seek_music(music_ptr: *mut Music, time: f64) -> bool {
    if music_ptr.is_null() {
        return false;
    }
    let music = unsafe { music_ptr.as_mut().unwrap() };
    music.seek_to(time).is_ok()
}

#[no_mangle]
pub extern "C" fn set_music_volume(music_ptr: *mut Music, volume: f32) -> bool {
    if music_ptr.is_null() {
        return false;
    }
    let music = unsafe { music_ptr.as_mut().unwrap() };
    music.set_amplifier(volume).is_ok()
}

#[no_mangle]
pub extern "C" fn get_music_position(music_ptr: *mut Music) -> f64 {
    if music_ptr.is_null() {
        return 0.0;
    }
    let music = unsafe { music_ptr.as_mut().unwrap() };
    music.position()
}

#[no_mangle]
pub extern "C" fn get_audio_clip_duration(clip_ptr: *mut AudioClip) -> f64 {
    if clip_ptr.is_null() {
        return 0.0;
    }
    let clip = unsafe { &*clip_ptr };
    clip.length()
}

#[no_mangle]
pub extern "C" fn destroy_manager(manager_ptr: *mut AudioManager) {
    if !manager_ptr.is_null() {
        unsafe {
            let _ = Box::from_raw(manager_ptr);
        };
    }
}

#[no_mangle]
pub extern "C" fn destroy_clip(clip_ptr: *mut AudioClip) {
    if !clip_ptr.is_null() {
        unsafe {
            let _ = Box::from_raw(clip_ptr);
        };
    }
}

#[no_mangle]
pub extern "C" fn destroy_sfx(sfx_ptr: *mut Sfx) {
    if !sfx_ptr.is_null() {
        unsafe {
            let _ = Box::from_raw(sfx_ptr);
        };
    }
}

#[no_mangle]
pub extern "C" fn destroy_music(music_ptr: *mut Music) {
    if !music_ptr.is_null() {
        unsafe {
            let _ = Box::from_raw(music_ptr);
        };
    }
}