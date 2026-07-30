use jiff::civil::Date;
use jiff::tz::TimeZone;
use takusu_types::Point;

pub use takusu_types::{SLOT_MINUTES, TimeOfDay};

pub fn point_to_date(point: Point, tz: &TimeZone) -> Option<Date> {
    let ts = point.to_timestamp(SLOT_MINUTES as u16)?;
    Some(ts.to_zoned(tz.clone()).date())
}

pub fn date_time_to_point(date: Date, time: &TimeOfDay, tz: &TimeZone) -> Option<Point> {
    let dt = date.at(time.hour() as i8, time.minute() as i8, 0, 0);
    let ts = tz.to_timestamp(dt).ok()?;
    Some(Point::from_timestamp(ts, SLOT_MINUTES as u16))
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
