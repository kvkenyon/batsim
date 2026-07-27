//! In-memory telemetry retention: per-home raw-tick ring buffers plus
//! one-minute rollups, with on-demand bucket aggregation for coarser
//! resolutions (including 5-minute settlement intervals).
//!
//! Everything here lives on the engine thread; queries arrive over the
//! engine command channel, so no locks are needed.

use std::collections::VecDeque;

use batsim_core::telemetry::HomeTruth;

/// One retained raw tick.
#[derive(Debug, Clone, Copy)]
pub struct TickPoint {
    /// Unix seconds of the tick.
    pub unix: u64,
    /// Tick index.
    pub tick: u64,
    /// Mean SOC.
    pub soc: f64,
    /// Battery AC power (W; + discharge).
    pub batt_w: f64,
    /// PV AC power (W).
    pub pv_w: f64,
    /// Load power (W).
    pub load_w: f64,
    /// Grid exchange (W; + import).
    pub grid_w: f64,
    /// Real-time price ($/MWh) at this tick.
    pub price: f64,
}

impl TickPoint {
    /// Build from a truth record plus the tick's price.
    #[must_use]
    pub fn from_truth(t: &HomeTruth, price: f64) -> Self {
        Self {
            unix: t.unix_time_s,
            tick: t.tick,
            soc: t.soc_mean,
            batt_w: t.p_batt_ac_w,
            pv_w: t.p_pv_ac_w,
            load_w: t.p_load_w,
            grid_w: t.p_grid_w,
            price,
        }
    }
}

/// A closed one-minute rollup.
#[derive(Debug, Clone, Copy)]
struct MinuteBucket {
    start_unix: u64,
    n: u32,
    soc_last: f64,
    batt_sum: f64,
    pv_sum: f64,
    load_sum: f64,
    grid_sum: f64,
    price_sum: f64,
}

impl MinuteBucket {
    fn new(start_unix: u64) -> Self {
        Self {
            start_unix,
            n: 0,
            soc_last: 0.0,
            batt_sum: 0.0,
            pv_sum: 0.0,
            load_sum: 0.0,
            grid_sum: 0.0,
            price_sum: 0.0,
        }
    }

    fn add(&mut self, p: &TickPoint) {
        self.n += 1;
        self.soc_last = p.soc;
        self.batt_sum += p.batt_w;
        self.pv_sum += p.pv_w;
        self.load_sum += p.load_w;
        self.grid_sum += p.grid_w;
        self.price_sum += p.price;
    }
}

/// A single field column in bucket-aggregate form.
#[derive(Debug, Clone, Copy, Default)]
pub struct BucketValue {
    /// Mean power over the bucket (W), or mean price ($/MWh).
    pub mean: f64,
    /// Last SOC in the bucket.
    pub last: f64,
    /// Samples folded in.
    pub n: u32,
}

/// One aggregated bucket for one home.
#[derive(Debug, Clone, Copy, Default)]
pub struct HomeBucket {
    /// Bucket start (unix seconds).
    pub start_unix: u64,
    /// SOC (last).
    pub soc: f64,
    /// Battery power mean (W).
    pub batt_w: f64,
    /// PV power mean (W).
    pub pv_w: f64,
    /// Load power mean (W).
    pub load_w: f64,
    /// Grid power mean (W).
    pub grid_w: f64,
    /// Price mean ($/MWh).
    pub price: f64,
}

/// Per-home retention series.
#[derive(Debug, Default)]
struct HomeSeries {
    raw: VecDeque<TickPoint>,
    minutes: VecDeque<MinuteBucket>,
    open: Option<MinuteBucket>,
}

impl HomeSeries {
    fn push(&mut self, p: TickPoint, raw_cap: usize, rollup_cap: usize) {
        self.raw.push_back(p);
        while self.raw.len() > raw_cap {
            self.raw.pop_front();
        }
        let minute = p.unix / 60 * 60;
        match &mut self.open {
            Some(b) if b.start_unix == minute => b.add(&p),
            Some(b) => {
                let closed = *b;
                self.minutes.push_back(closed);
                while self.minutes.len() > rollup_cap {
                    self.minutes.pop_front();
                }
                let mut nb = MinuteBucket::new(minute);
                nb.add(&p);
                *b = nb;
            }
            slot @ None => {
                let mut b = MinuteBucket::new(minute);
                b.add(&p);
                *slot = Some(b);
            }
        }
    }

    /// All closed minutes plus the open one, oldest first.
    fn all_minutes(&self) -> impl Iterator<Item = &MinuteBucket> {
        self.minutes.iter().chain(self.open.as_ref())
    }
}

/// The telemetry store.
#[derive(Debug)]
pub struct TelemetryStore {
    raw_cap: usize,
    rollup_cap: usize,
    homes: std::collections::HashMap<u64, HomeSeries>,
}

impl TelemetryStore {
    /// Create with the given ring capacities.
    #[must_use]
    pub fn new(raw_cap: usize, rollup_cap: usize) -> Self {
        Self {
            raw_cap,
            rollup_cap,
            homes: std::collections::HashMap::new(),
        }
    }

    /// Append one tick for one home.
    pub fn push(&mut self, home_idx: u64, point: TickPoint) {
        self.homes
            .entry(home_idx)
            .or_default()
            .push(point, self.raw_cap, self.rollup_cap);
    }

    /// Drop all retained data (scenario rebind).
    pub fn clear(&mut self) {
        self.homes.clear();
    }

    /// Latest raw point for a home.
    #[must_use]
    pub fn latest(&self, home_idx: u64) -> Option<TickPoint> {
        self.homes.get(&home_idx)?.raw.back().copied()
    }

    /// Aggregated buckets for one home over `[from, to)` unix seconds at
    /// `bucket_s` resolution.
    #[must_use]
    pub fn home_buckets(
        &self,
        home_idx: u64,
        from: u64,
        to: u64,
        bucket_s: u64,
    ) -> Vec<HomeBucket> {
        let Some(series) = self.homes.get(&home_idx) else {
            return Vec::new();
        };
        let mut out: Vec<HomeBucket> = Vec::new();
        if bucket_s <= 60 {
            // Straight from raw ticks or closed minutes.
            if bucket_s == 1 {
                for p in &series.raw {
                    if p.unix >= from && p.unix < to {
                        out.push(HomeBucket {
                            start_unix: p.unix,
                            soc: p.soc,
                            batt_w: p.batt_w,
                            pv_w: p.pv_w,
                            load_w: p.load_w,
                            grid_w: p.grid_w,
                            price: p.price,
                        });
                    }
                }
            } else {
                for b in series.all_minutes() {
                    if b.start_unix >= from && b.start_unix < to && b.n > 0 {
                        let n = f64::from(b.n);
                        out.push(HomeBucket {
                            start_unix: b.start_unix,
                            soc: b.soc_last,
                            batt_w: b.batt_sum / n,
                            pv_w: b.pv_sum / n,
                            load_w: b.load_sum / n,
                            grid_w: b.grid_sum / n,
                            price: b.price_sum / n,
                        });
                    }
                }
            }
            return out;
        }
        // Coarser than a minute: fold minutes into aligned buckets.
        let mut cur_start = 0u64;
        let mut acc = HomeBucket::default();
        let mut n = 0u32;
        let mut first = true;
        for b in series.all_minutes() {
            if b.start_unix < from || b.start_unix >= to || b.n == 0 {
                continue;
            }
            let bucket = b.start_unix / bucket_s * bucket_s;
            if first || bucket != cur_start {
                if !first && n > 0 {
                    out.push(Self::finish_bucket(acc, n, cur_start));
                }
                first = false;
                cur_start = bucket;
                acc = HomeBucket::default();
                n = 0;
            }
            let bn = f64::from(b.n);
            acc.soc = b.soc_last;
            acc.batt_w += b.batt_sum / bn;
            acc.pv_w += b.pv_sum / bn;
            acc.load_w += b.load_sum / bn;
            acc.grid_w += b.grid_sum / bn;
            acc.price += b.price_sum / bn;
            n += 1;
        }
        if !first && n > 0 {
            out.push(Self::finish_bucket(acc, n, cur_start));
        }
        out
    }

    fn finish_bucket(acc: HomeBucket, n: u32, start: u64) -> HomeBucket {
        let nf = f64::from(n);
        HomeBucket {
            start_unix: start,
            soc: acc.soc,
            batt_w: acc.batt_w / nf,
            pv_w: acc.pv_w / nf,
            load_w: acc.load_w / nf,
            grid_w: acc.grid_w / nf,
            price: acc.price / nf,
        }
    }
}
