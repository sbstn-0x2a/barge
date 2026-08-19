//! Bandbreiten-Drossel (§6.1).
//!
//! Kumulatives Pacing: nach jedem Chunk wird verglichen, wie viel Zeit für die
//! bisher geschriebene Bytemenge *mindestens* hätte vergehen müssen. Sind wir
//! schneller als erlaubt, wird die Differenz verschlafen. Sind wir langsamer
//! (z. B. durch eine `fsync`-Pause), wird **nicht** nachgeholt — dadurch ist
//! die eingestellte Rate eine Obergrenze, kein Sollwert (§6.1). Genau das ist
//! gewollt: Nachholen nach dem Sync erzeugt die Lastspitze, die das Tool
//! vermeiden soll.

use std::time::{Duration, Instant};

pub struct Throttle {
    /// Ziel-Obergrenze in Bytes/s. `0` bedeutet unbegrenzt.
    rate: u64,
    start: Instant,
    written: u64,
}

impl Throttle {
    pub fn new(rate_bytes_per_sec: u64) -> Self {
        Throttle {
            rate: rate_bytes_per_sec,
            start: Instant::now(),
            written: 0,
        }
    }

    /// Verbucht `n` geschriebene Bytes und schläft ggf., um die Obergrenze zu
    /// halten.
    pub fn account(&mut self, n: u64) {
        self.written += n;
        if self.rate == 0 {
            return;
        }
        let should = Duration::from_secs_f64(self.written as f64 / self.rate as f64);
        let actual = self.start.elapsed();
        if should > actual {
            std::thread::sleep(should - actual);
        }
    }

    /// Insgesamt verbuchte Bytes. (Für Fortschritts-/UI-Nutzung ab Stufe 5.)
    #[allow(dead_code)]
    pub fn written(&self) -> u64 {
        self.written
    }

    /// Gemessene Durchschnittsrate in MB/s (dezimal, wie im Handout §6.1).
    pub fn measured_mbps(&self) -> f64 {
        let secs = self.start.elapsed().as_secs_f64();
        if secs <= 0.0 {
            0.0
        } else {
            self.written as f64 / secs / 1_000_000.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begrenzt_die_rate() {
        // 10 MB bei 50 MB/s müssen mindestens ~0,2 s dauern.
        let rate = 50 * 1_000_000;
        let mut th = Throttle::new(rate);
        let start = Instant::now();
        for _ in 0..10 {
            th.account(1_000_000); // 1 MB
        }
        let elapsed = start.elapsed().as_secs_f64();
        assert!(
            elapsed >= 0.18,
            "zu schnell: {elapsed:.3}s (erwartet >= ~0,2s)"
        );
        // Nie schneller als die Obergrenze.
        assert!(th.measured_mbps() <= 50.0 + 1.0);
    }

    #[test]
    fn unbegrenzt_schlaeft_nicht() {
        let mut th = Throttle::new(0);
        let start = Instant::now();
        for _ in 0..1000 {
            th.account(1_000_000);
        }
        assert!(start.elapsed().as_secs_f64() < 0.1);
        assert_eq!(th.written(), 1_000_000_000);
    }
}
