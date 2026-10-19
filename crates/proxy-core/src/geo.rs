use std::net::IpAddr;
use std::path::Path;

/// The flag of a country, as an emoji.
///
/// A flag is two regional indicator symbols, one per letter of the code, so it
/// is derived rather than looked up: there is no table to keep in step with
/// the codes.
pub fn flag(code: &str) -> Option<String> {
    let code = code.trim().to_ascii_uppercase();
    if code.len() != 2 || !code.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }

    let mut flag = String::new();
    for letter in code.chars() {
        // The regional indicator symbols start one past the basic latin block.
        let point = 0x1F1E6 + (letter as u32 - 'A' as u32);
        flag.push(char::from_u32(point)?);
    }
    Some(flag)
}

/// One range of addresses and the country it belongs to.
#[derive(Debug, Clone, Copy)]
struct Range {
    start: u32,
    end: u32,
    /// The country, as an index into the list of codes.
    country: u16,
}

/// The countries an address belongs to.
///
/// The database is a list of ranges: the first and last address of a range and
/// the country it is in. It is held sorted and searched by bisection, so a
/// lookup costs nothing and the whole thing takes a few megabytes.
#[derive(Debug, Default)]
pub struct GeoDatabase {
    ranges: Vec<Range>,
    /// The country codes the ranges refer to.
    ///
    /// The codes are held once and the ranges point into them, so a database
    /// with a quarter of a million ranges does not carry a quarter of a million
    /// copies of a two-letter code.
    codes: Vec<String>,
    /// When it was loaded.
    loaded_at: Option<std::time::SystemTime>,
}

impl GeoDatabase {
    /// Load a database from a file.
    ///
    /// The file is the format the free country databases are published in:
    /// `first_address,last_address,country_code`, one range per line.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let body = std::fs::read_to_string(path)?;
        let mut database = Self::from_csv(&body);
        database.loaded_at = Some(std::time::SystemTime::now());
        Ok(database)
    }

    /// Build a database from the text of a country list.
    pub fn from_csv(body: &str) -> Self {
        let mut ranges = Vec::new();
        let mut codes: Vec<String> = Vec::new();

        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let mut fields = line.split(',');
            let (Some(first), Some(last), Some(code)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };

            let (Some(first), Some(last)) = (parse_ipv4(first.trim()), parse_ipv4(last.trim()))
            else {
                continue;
            };

            let code = code.trim().to_ascii_uppercase();
            if code.len() != 2 || !code.chars().all(|c| c.is_ascii_alphabetic()) {
                continue;
            }

            // Every code the database names is kept. A country the code list
            // does not know is still a country, and dropping it would leave a
            // hole in the coverage.
            let index = match codes.iter().position(|known| *known == code) {
                Some(index) => index,
                None => {
                    codes.push(code);
                    codes.len() - 1
                }
            };

            ranges.push(Range {
                start: first,
                end: last,
                country: index as u16,
            });
        }

        ranges.sort_unstable_by_key(|range| range.start);
        Self {
            ranges,
            codes,
            loaded_at: None,
        }
    }

    /// How many ranges the database holds.
    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// When the database was loaded.
    pub fn loaded_at(&self) -> Option<std::time::SystemTime> {
        self.loaded_at
    }

    /// The country an address is in, as an ISO 3166-1 alpha-2 code.
    pub fn lookup(&self, ip: IpAddr) -> Option<&str> {
        let address = match ip {
            IpAddr::V4(v4) => u32::from(v4),
            // The databases here cover IPv4. An IPv6 exit is reported as
            // unknown rather than guessed at.
            IpAddr::V6(_) => return None,
        };

        // The last range that starts at or before the address.
        let index = self
            .ranges
            .partition_point(|range| range.start <= address)
            .checked_sub(1)?;

        let range = self.ranges[index];
        if address <= range.end {
            self.codes.get(range.country as usize).map(String::as_str)
        } else {
            None
        }
    }
}

/// Parse an IPv4 address into a number, so ranges can be compared.
fn parse_ipv4(text: &str) -> Option<u32> {
    let mut octets = [0u8; 4];
    let mut parts = text.split('.');

    for octet in octets.iter_mut() {
        let part = parts.next()?;
        *octet = part.parse::<u8>().ok()?;
    }

    if parts.next().is_some() {
        return None;
    }

    Some(u32::from_be_bytes(octets))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
1.0.1.0,1.0.3.255,CN
8.8.8.0,8.8.8.255,US
185.0.0.0,185.255.255.255,DE
91.0.0.0,91.255.255.255,RU
";

    #[test]
    fn finds_the_country_of_an_address() {
        let database = GeoDatabase::from_csv(SAMPLE);

        assert_eq!(database.lookup("8.8.8.8".parse().unwrap()), Some("US"));
        assert_eq!(database.lookup("1.0.2.3".parse().unwrap()), Some("CN"));
        assert_eq!(database.lookup("185.10.20.30".parse().unwrap()), Some("DE"));
        assert_eq!(database.lookup("91.1.2.3".parse().unwrap()), Some("RU"));
    }

    #[test]
    fn an_address_outside_every_range_has_no_country() {
        let database = GeoDatabase::from_csv(SAMPLE);
        assert_eq!(database.lookup("100.64.0.1".parse().unwrap()), None);
    }

    #[test]
    fn an_ipv6_address_has_no_country() {
        let database = GeoDatabase::from_csv(SAMPLE);
        assert_eq!(database.lookup("2001:db8::1".parse().unwrap()), None);
    }

    #[test]
    fn the_codes_are_held_once() {
        let database = GeoDatabase::from_csv(
            "1.0.0.0,1.0.0.255,CN\n2.0.0.0,2.0.0.255,CN\n3.0.0.0,3.0.0.255,US\n",
        );
        assert_eq!(database.codes.len(), 2);
    }

    #[test]
    fn a_flag_is_derived_from_the_code() {
        assert_eq!(flag("RU").unwrap(), "🇷🇺");
        assert_eq!(flag("us").unwrap(), "🇺🇸");
        assert_eq!(flag("DE").unwrap(), "🇩🇪");
        assert_eq!(flag("XYZ"), None);
        assert_eq!(flag("R"), None);
    }

    #[test]
    fn a_code_the_database_names_is_kept_even_when_unknown() {
        // The engine keeps the code and the platform names it, so a country
        // the engine has never heard of is still reported.
        let database = GeoDatabase::from_csv("5.1.0.0,5.1.0.255,ZZ\n");
        assert_eq!(database.lookup("5.1.0.1".parse().unwrap()), Some("ZZ"));
    }

    #[test]
    fn the_ranges_are_sorted_for_the_search() {
        let database = GeoDatabase::from_csv("9.0.0.0,9.0.0.255,FR\n1.0.0.0,1.0.0.255,CN\n");
        assert_eq!(database.lookup("1.0.0.1".parse().unwrap()), Some("CN"));
        assert_eq!(database.lookup("9.0.0.1".parse().unwrap()), Some("FR"));
    }
}
