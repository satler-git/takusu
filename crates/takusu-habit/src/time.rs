use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use takusu_core::Point;

pub use takusu_core::SLOT_MINUTES;
#[allow(dead_code)]
pub const SLOTS_PER_HOUR: i64 = 12;
#[allow(dead_code)]
pub const SLOTS_PER_DAY: i64 = 288;

#[derive(Debug, Clone, Copy)]
pub struct TimeOfDay {
    pub hour: u8,
    pub minute: u8,
}

impl TimeOfDay {
    pub fn new(hour: u8, minute: u8) -> Option<Self> {
        if hour > 23 || minute > 59 {
            return None;
        }
        // snap minute to 5-min slots
        let snapped = (minute as i64 / SLOT_MINUTES * SLOT_MINUTES) as u8;
        Some(Self {
            hour,
            minute: snapped,
        })
    }

    pub fn hour(self) -> i8 {
        self.hour as i8
    }

    pub fn minute(self) -> i8 {
        self.minute as i8
    }
}

pub fn point_to_date(point: Point, tz: &TimeZone) -> Option<Date> {
    let seconds = point.0.checked_mul(SLOT_MINUTES)?.checked_mul(60)?;
    let ts = Timestamp::from_second(seconds).ok()?;
    Some(ts.to_zoned(tz.clone()).date())
}

pub fn date_time_to_point(date: Date, time: &TimeOfDay, tz: &TimeZone) -> Option<Point> {
    let dt = date.at(time.hour(), time.minute(), 0, 0);
    let ts = tz.to_timestamp(dt).ok()?;
    Some(Point::from_timestamp(ts, SLOT_MINUTES as u16))
}

#[allow(dead_code)]
pub fn point_to_day_number(point: Point, tz: &TimeZone) -> i64 {
    let date = point_to_date(point, tz).unwrap_or(Date::MAX);
    date_to_day_number(date)
}

pub fn date_to_day_number(date: Date) -> i64 {
    let y = date.year() as i64;
    let m = date.month() as i64;
    let d = date.day() as i64;
    let a = (14 - m) / 12;
    let y2 = y + 4800 - a;
    let m2 = m + 12 * a - 3;
    d + (153 * m2 + 2) / 5 + 365 * y2 + y2 / 4 - y2 / 100 + y2 / 400 - 32045
}
