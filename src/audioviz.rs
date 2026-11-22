//! Espectro de lo que suena en el PC (captura en bucle de la salida por defecto con WASAPI y una FFT
//! pequeña). Solo trabaja mientras alguien lo mira (`want`); el resto del tiempo el hilo duerme.

use super::*;
use std::sync::atomic::AtomicBool;
use windows::Win32::Media::Audio::*;

/// Bandas del espectro (de graves a agudos, en escala logarítmica).
pub const BANDS: usize = 36;
const N: usize = 1024;

/// Niveles 0..1 de cada banda, ya suavizados.
pub static LEVELS: Mutex<[f32; BANDS]> = Mutex::new([0.0; BANDS]);
static WANTED: AtomicBool = AtomicBool::new(false);
static LAST_WANT: Mutex<Option<Instant>> = Mutex::new(None);
static STARTED: std::sync::Once = std::sync::Once::new();

/// El widget lo pide mientras se ve (si deja de pedirlo 2 s, la captura se para).
pub fn want() {
    WANTED.store(true, Ordering::Relaxed);
    *LAST_WANT.lock().unwrap() = Some(Instant::now());
    STARTED.call_once(|| { std::thread::spawn(worker); });
}

fn wanted() -> bool {
    let fresh = LAST_WANT.lock().unwrap().is_some_and(|t| t.elapsed() < Duration::from_secs(2));
    if !fresh { WANTED.store(false, Ordering::Relaxed); }
    fresh
}

/// FFT radix-2 en el sitio (re, im de longitud potencia de 2).
pub fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 { j ^= bit; bit >>= 1; }
        j |= bit;
        if i < j { re.swap(i, j); im.swap(i, j); }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f32::consts::PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for i in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (i + k, i + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr; im[b] = im[a] - ti;
                re[a] += tr; im[a] += ti;
                let nr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = nr;
            }
        }
        len <<= 1;
    }
}

/// Bandas logarítmicas (40 Hz … 16 kHz) a partir de las magnitudes de la FFT, en 0..1.
pub fn bands(mag: &[f32], rate: f32) -> [f32; BANDS] {
    let mut out = [0.0f32; BANDS];
    let bin_hz = rate / (2.0 * mag.len() as f32);
    let (lo, hi) = (40.0f32.ln(), 16000.0f32.ln());
    for (b, o) in out.iter_mut().enumerate() {
        let f0 = (lo + (hi - lo) * b as f32 / BANDS as f32).exp();
        let f1 = (lo + (hi - lo) * (b + 1) as f32 / BANDS as f32).exp();
        let (i0, i1) = (((f0 / bin_hz) as usize).max(1), ((f1 / bin_hz) as usize).max(1));
        let i1 = i1.max(i0 + 1).min(mag.len());
        let m = mag[i0.min(mag.len() - 1)..i1].iter().cloned().fold(0.0f32, f32::max);
        // A decibelios: -60 dB = 0, 0 dB = 1; los agudos se realzan un poco (si no, apenas se ven)
        let db = 20.0 * (m + 1e-9).log10() + 6.0 * (b as f32 / BANDS as f32);
        *o = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    }
    out
}

fn worker() {
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
    loop {
        if !wanted() {
            *LEVELS.lock().unwrap() = [0.0; BANDS];
            std::thread::sleep(Duration::from_millis(400));
            continue;
        }
        // Modo demo: un espectro inventado que se mueve como música (para capturas sin sonar nada)
        if sys::demo() {
            let t = Instant::now();
            while wanted() {
                let s = t.elapsed().as_secs_f32();
                let mut l = [0.0f32; BANDS];
                for (i, v) in l.iter_mut().enumerate() {
                    let x = i as f32 / BANDS as f32;
                    let beat = (0.5 + 0.5 * (s * 7.5).sin()).powf(3.0);
                    *v = ((0.85 - 0.55 * x) * (0.55 + 0.45 * (s * 3.1 + i as f32 * 0.7).sin().abs()) + 0.25 * beat * (1.0 - x)).clamp(0.0, 1.0);
                }
                *LEVELS.lock().unwrap() = l;
                std::thread::sleep(Duration::from_millis(16));
            }
            continue;
        }
        // Si algo falla (cambio de salida, dispositivo quitado), se vuelve a abrir en un momento
        let _ = capture();
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn capture() -> Option<()> {
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).ok()?;
        let fmt = client.GetMixFormat().ok()?;
        let (ch, rate, bits) = ((*fmt).nChannels as usize, (*fmt).nSamplesPerSec as f32, (*fmt).wBitsPerSample);
        let r = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 200_000, 0, fmt, None);
        CoTaskMemFree(Some(fmt as _));
        r.ok()?;
        let cap: IAudioCaptureClient = client.GetService().ok()?;
        client.Start().ok()?;
        let mut ring = vec![0.0f32; N];
        let mut pos = 0usize;
        let mut smooth = [0.0f32; BANDS];
        let win: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (N - 1) as f32).cos()).collect();
        let mut last = Instant::now();
        while wanted() {
            std::thread::sleep(Duration::from_millis(16));
            loop {
                let n = cap.GetNextPacketSize().ok()?;
                if n == 0 { break; }
                let mut data = std::ptr::null_mut();
                let (mut frames, mut flags) = (0u32, 0u32);
                cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None).ok()?;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                for f in 0..frames as usize {
                    // Mezcla a mono (formato de mezcla: float de 32 bits casi siempre)
                    let mut v = 0.0f32;
                    if !silent && bits == 32 {
                        let p = data as *const f32;
                        for c in 0..ch { v += *p.add(f * ch + c); }
                        v /= ch.max(1) as f32;
                    } else if !silent && bits == 16 {
                        let p = data as *const i16;
                        for c in 0..ch { v += *p.add(f * ch + c) as f32 / 32768.0; }
                        v /= ch.max(1) as f32;
                    }
                    ring[pos] = v;
                    pos = (pos + 1) % N;
                }
                cap.ReleaseBuffer(frames).ok()?;
            }
            let (mut re, mut im) = (vec![0.0f32; N], vec![0.0f32; N]);
            for i in 0..N { re[i] = ring[(pos + i) % N] * win[i]; }
            fft(&mut re, &mut im);
            let mag: Vec<f32> = (0..N / 2).map(|i| (re[i] * re[i] + im[i] * im[i]).sqrt() * 2.0 / N as f32 * 2.0).collect();
            let b = bands(&mag, rate);
            // Sube rápido y cae despacio (como un vúmetro)
            let dt = last.elapsed().as_secs_f32().min(0.1);
            last = Instant::now();
            for i in 0..BANDS {
                let k = if b[i] > smooth[i] { 1.0 - (-dt / 0.03).exp() } else { 1.0 - (-dt / 0.22).exp() };
                smooth[i] += (b[i] - smooth[i]) * k;
            }
            *LEVELS.lock().unwrap() = smooth;
        }
        let _ = client.Stop();
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_de_un_tono() {
        // Un seno de 1 kHz a 48 kHz cae en su banda y apenas en las demás
        let rate = 48000.0;
        let (mut re, mut im) = ((0..N).map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / rate).sin()).collect::<Vec<_>>(), vec![0.0; N]);
        fft(&mut re, &mut im);
        let mag: Vec<f32> = (0..N / 2).map(|i| (re[i] * re[i] + im[i] * im[i]).sqrt() * 2.0 / N as f32).collect();
        let peak = mag.iter().enumerate().fold((0, 0.0f32), |a, (i, m)| if *m > a.1 { (i, *m) } else { a }).0;
        let hz = peak as f32 * rate / N as f32;
        assert!((hz - 1000.0).abs() < 60.0, "pico en {hz} Hz");
        let b = bands(&mag, rate);
        let top = b.iter().enumerate().fold((0, 0.0f32), |a, (i, v)| if *v > a.1 { (i, *v) } else { a }).0;
        // 1 kHz está hacia la mitad de 40 Hz … 16 kHz (escala logarítmica)
        assert!((14..=22).contains(&top), "banda {top}");
        // Silencio: todo a cero
        assert!(bands(&vec![0.0; N / 2], rate).iter().all(|v| *v == 0.0));
    }
}
