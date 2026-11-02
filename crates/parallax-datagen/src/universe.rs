//! The securities, books and funds that generated rows refer to.
//!
//! A universe is a pure function of its seed, size and hostility, so positions,
//! trades and prices built from the same seed agree on every security.

use std::collections::HashSet;

use jiff::civil::{Date, date};

use crate::decimal::from_f64;
use crate::dist::{Weighted, log_normal, standard_normal};
use crate::error::OptionsError;
use crate::rng::{Rng, Stream};

/// The kind of instrument a security is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetClass {
    /// Common stock.
    Equity,
    /// A bond quoted per 100 of face value.
    Bond,
    /// An exchange-traded future.
    Future,
    /// A listed option.
    Option,
}

impl AssetClass {
    /// The label used in generated data.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Equity => "Equity",
            Self::Bond => "Bond",
            Self::Future => "Future",
            Self::Option => "Option",
        }
    }
}

/// Terms that only bonds have.
#[derive(Clone, Debug)]
pub struct BondTerms {
    /// Annual coupon in percent, a multiple of 0.125.
    pub coupon_rate: f64,
    /// Maturity date.
    pub maturity: Date,
    /// Modified duration in years.
    pub duration: f64,
    /// Yield to maturity in percent.
    pub yield_to_maturity: f64,
    /// Credit rating.
    pub rating: &'static str,
}

/// One security.
#[derive(Clone, Debug)]
pub struct Security {
    /// Identifier shown in grids. Hostile universes include Unicode and blank tickers.
    pub ticker: String,
    /// Descriptive name.
    pub name: String,
    /// Instrument kind.
    pub asset_class: AssetClass,
    /// GICS-style sector, absent for futures.
    pub sector: Option<&'static str>,
    /// Industry within the sector, absent for futures.
    pub industry: Option<&'static str>,
    /// ISO 3166 country code.
    pub country: &'static str,
    /// Region of the country.
    pub region: &'static str,
    /// ISO 4217 currency of the price.
    pub currency: &'static str,
    /// Rate converting one unit of `currency` to USD, scale 8.
    pub fx_rate_to_usd: i128,
    /// Typical price in `currency`.
    pub base_price: f64,
    /// Decimal places the price is quoted to.
    pub price_decimals: u32,
    /// Contract multiplier, scale 2: 1.00 for equities, 0.01 for bonds (priced per
    /// 100 of face), the contract size for futures and options.
    pub multiplier: i128,
    /// Market beta.
    pub beta: f64,
    /// Annualized volatility.
    pub volatility: f64,
    /// Average daily volume in shares, face value or contracts.
    pub average_daily_volume: f64,
    /// Bond terms, for bonds only.
    pub bond: Option<BondTerms>,
    /// Expiry, for futures and options only.
    pub expiry: Option<Date>,
}

/// A fund.
#[derive(Clone, Debug)]
pub struct Fund {
    /// Fund name.
    pub name: &'static str,
    /// Net asset value in USD, used for weights.
    pub nav_usd: f64,
}

/// A strategy within a fund.
#[derive(Clone, Debug)]
pub struct Strategy {
    /// Strategy name.
    pub name: &'static str,
    /// Index into [`Universe::funds`].
    pub fund: usize,
}

/// A book run by one portfolio manager within a strategy.
#[derive(Clone, Debug)]
pub struct Book {
    /// Book code.
    pub name: String,
    /// Portfolio manager's name.
    pub portfolio_manager: String,
    /// Index into [`Universe::strategies`].
    pub strategy: usize,
}

/// Securities, funds, strategies and books, with the frequencies rows draw them by.
#[derive(Clone, Debug)]
pub struct Universe {
    /// Securities, most frequently held first.
    pub securities: Vec<Security>,
    /// Zipf frequency over `securities`.
    pub security_frequency: Weighted,
    /// Funds.
    pub funds: Vec<Fund>,
    /// Strategies.
    pub strategies: Vec<Strategy>,
    /// Books.
    pub books: Vec<Book>,
    /// Frequency over `books`.
    pub book_frequency: Weighted,
    /// Traders who book trades.
    pub traders: Vec<String>,
}

/// Sectors and their industries.
pub const SECTORS: [(&str, [&str; 3]); 11] = [
    (
        "Communication Services",
        ["Media", "Entertainment", "Telecom Services"],
    ),
    (
        "Consumer Discretionary",
        ["Retail", "Automobiles", "Hotels & Leisure"],
    ),
    (
        "Consumer Staples",
        ["Food Products", "Beverages", "Household Products"],
    ),
    ("Energy", ["Oil & Gas", "Energy Equipment", "Renewables"]),
    ("Financials", ["Banks", "Insurance", "Capital Markets"]),
    (
        "Health Care",
        ["Pharmaceuticals", "Biotechnology", "Health Care Providers"],
    ),
    (
        "Industrials",
        ["Aerospace & Defense", "Machinery", "Transportation"],
    ),
    (
        "Information Technology",
        ["Software", "Semiconductors", "Hardware"],
    ),
    (
        "Materials",
        ["Chemicals", "Metals & Mining", "Construction Materials"],
    ),
    (
        "Real Estate",
        ["REITs", "Real Estate Services", "Developers"],
    ),
    (
        "Utilities",
        ["Electric Utilities", "Gas Utilities", "Water Utilities"],
    ),
];

/// Countries as `(code, region, currency, USD rate at scale 8, weight)`.
pub const COUNTRIES: [(&str, &str, &str, i128, f64); 14] = [
    ("US", "North America", "USD", 100_000_000, 55.0),
    ("CA", "North America", "CAD", 72_310_000, 4.0),
    ("GB", "Europe", "GBP", 129_104_500, 6.0),
    ("DE", "Europe", "EUR", 108_532_100, 4.0),
    ("FR", "Europe", "EUR", 108_532_100, 4.0),
    ("NL", "Europe", "EUR", 108_532_100, 2.0),
    ("CH", "Europe", "CHF", 113_350_000, 3.0),
    ("SE", "Europe", "SEK", 9_450_000, 1.5),
    ("JP", "Asia Pacific", "JPY", 659_820, 7.0),
    ("HK", "Asia Pacific", "HKD", 12_865_000, 3.0),
    ("AU", "Asia Pacific", "AUD", 65_810_000, 3.0),
    ("IN", "Asia Pacific", "INR", 1_196_000, 2.0),
    ("KR", "Asia Pacific", "KRW", 72_400, 2.0),
    ("BR", "Latin America", "BRL", 18_350_000, 1.5),
];

const FUNDS: [&str; 8] = [
    "Atlas Global Macro",
    "Meridian Long/Short",
    "Harbor Credit Opportunities",
    "Summit Quant Equity",
    "Cobalt Event Driven",
    "Granite Multi-Strategy",
    "Northwind Rates",
    "Juniper Emerging Markets",
];

const STRATEGIES: [&str; 12] = [
    "Long/Short Equity",
    "Event Driven",
    "Merger Arbitrage",
    "Statistical Arbitrage",
    "Quant Momentum",
    "Credit Long/Short",
    "Distressed",
    "Rates Relative Value",
    "Volatility",
    "Global Macro",
    "Convertible Arbitrage",
    "Emerging Markets",
];

const FIRST_NAMES: [&str; 16] = [
    "Ana", "Ben", "Chloe", "Dev", "Elena", "Femi", "Grace", "Hiro", "Isla", "Jonas", "Kira",
    "Liam", "Maya", "Nikhil", "Olga", "Priya",
];

const LAST_NAMES: [&str; 16] = [
    "Abbott", "Barros", "Chen", "Dubois", "Eriksen", "Fischer", "Gupta", "Haddad", "Ito", "Jensen",
    "Kowalski", "Larsen", "Moreau", "Nakamura", "Okafor", "Petrov",
];

const NAME_WORDS: [&str; 24] = [
    "Apex", "Blue", "Cedar", "Delta", "Echo", "Falcon", "Golden", "Harbor", "Iron", "Jade",
    "Keystone", "Lumen", "Maple", "Nova", "Orion", "Pioneer", "Quantum", "River", "Silver",
    "Titan", "Union", "Vertex", "Willow", "Zenith",
];

const NAME_SUFFIXES: [&str; 6] = ["Inc", "Corp", "Holdings", "Group", "plc", "AG"];

const RATINGS: [(&str, f64); 14] = [
    ("AAA", 3.0),
    ("AA+", 2.0),
    ("AA", 3.0),
    ("AA-", 4.0),
    ("A+", 6.0),
    ("A", 8.0),
    ("A-", 9.0),
    ("BBB+", 12.0),
    ("BBB", 14.0),
    ("BBB-", 12.0),
    ("BB+", 8.0),
    ("BB", 7.0),
    ("B", 8.0),
    ("CCC", 4.0),
];

const FUTURE_MULTIPLIERS: [i128; 4] = [5_000, 10_000, 2_500, 100_000];
const FUTURE_MONTH_CODES: [char; 4] = ['H', 'M', 'U', 'Z'];

/// Tickers that break naive code: strings that look like nulls or numbers, blank and
/// padded strings, combining marks that look identical to precomposed letters,
/// scripts, right-to-left text, emoji and invisible characters. The most dangerous
/// come first, so they land on the most frequently held securities.
pub const HOSTILE_TICKERS: [&str; 15] = [
    "NULL",
    "",
    "e\u{301}cole",
    "\u{e9}cole",
    "株式会社",
    "  PAD  ",
    "-0",
    "1e5",
    "a\u{200b}b",
    "ÄÖÜ",
    "Ελλάς",
    "שלום",
    "🚀MOON",
    "TAB\tX",
    "Ω",
];

/// Frequency ranks that receive hostile tickers, so they appear among both the most
/// and the least common securities.
const HOSTILE_RANKS: [usize; 15] = [
    0, 2, 5, 11, 23, 47, 97, 199, 401, 809, 1601, 3203, 6007, 8009, 9001,
];

impl Universe {
    /// Builds a universe of `securities` securities. Bond maturities and contract
    /// expiries fall after `reference`, the as-of date or last trade date of the
    /// dataset that uses the universe.
    ///
    /// # Errors
    ///
    /// Returns [`OptionsError::EmptyUniverse`] if `securities` is zero.
    pub fn new(
        seed: u64,
        securities: usize,
        hostile: bool,
        reference: Date,
    ) -> Result<Self, OptionsError> {
        if securities == 0 {
            return Err(OptionsError::EmptyUniverse);
        }
        let mut rng = Rng::for_key(seed, Stream::Universe, 0);
        let country_frequency = Weighted::new(&COUNTRIES.map(|c| c.4));
        let rating_frequency = Weighted::new(&RATINGS.map(|r| r.1));
        // Reserving the hostile tickers in both modes keeps generated roots from
        // colliding with them, and keeps calm and hostile universes identical apart
        // from the renamed securities.
        let mut used_tickers: HashSet<String> =
            HOSTILE_TICKERS.iter().map(|&t| t.to_owned()).collect();
        let mut list: Vec<Security> = (0..securities)
            .map(|_| {
                new_security(
                    &mut rng,
                    &country_frequency,
                    &rating_frequency,
                    &mut used_tickers,
                    reference,
                )
            })
            .collect();
        if hostile {
            for (rank, ticker) in HOSTILE_RANKS.iter().zip(HOSTILE_TICKERS) {
                if let Some(security) = list.get_mut(*rank) {
                    security.ticker = ticker.to_owned();
                }
            }
        }

        let funds: Vec<Fund> = FUNDS
            .iter()
            .map(|&name| Fund {
                name,
                nav_usd: log_normal(&mut rng, libm::log(2.0e9), 0.6),
            })
            .collect();
        let mut strategies = Vec::new();
        let mut books = Vec::new();
        let mut book_weights = Vec::new();
        let mut traders = Vec::new();
        for (fund_index, _) in funds.iter().enumerate() {
            let strategy_count = 3 + rng.index(4);
            // Stepping by 5, which shares no factor with 12, keeps a fund's strategy
            // names distinct.
            let first_strategy = rng.index(STRATEGIES.len());
            for k in 0..strategy_count {
                let strategy_index = strategies.len();
                strategies.push(Strategy {
                    name: STRATEGIES[(first_strategy + 5 * k) % STRATEGIES.len()],
                    fund: fund_index,
                });
                for _ in 0..1 + rng.index(3) {
                    let first = FIRST_NAMES[rng.index(FIRST_NAMES.len())];
                    let last = LAST_NAMES[rng.index(LAST_NAMES.len())];
                    let pm = format!("{first} {last}");
                    for _ in 0..1 + rng.index(3) {
                        let initials: String = [first, last]
                            .iter()
                            .filter_map(|n| n.chars().next())
                            .collect();
                        books.push(Book {
                            name: format!("{initials}-{:03}", books.len() + 1),
                            portfolio_manager: pm.clone(),
                            strategy: strategy_index,
                        });
                        book_weights.push(log_normal(&mut rng, 0.0, 0.8));
                    }
                }
                let first = FIRST_NAMES[rng.index(FIRST_NAMES.len())];
                let last = LAST_NAMES[rng.index(LAST_NAMES.len())];
                traders.push(format!("{first} {last}"));
            }
        }

        Ok(Self {
            security_frequency: Weighted::zipf(list.len(), 1.07),
            securities: list,
            funds,
            strategies,
            book_frequency: Weighted::new(&book_weights),
            books,
            traders,
        })
    }

    /// Returns the fund that `book` belongs to.
    #[must_use]
    pub fn fund_of(&self, book: usize) -> &Fund {
        &self.funds[self.strategies[self.books[book].strategy].fund]
    }
}

fn new_security(
    rng: &mut Rng,
    country_frequency: &Weighted,
    rating_frequency: &Weighted,
    used_tickers: &mut HashSet<String>,
    reference: Date,
) -> Security {
    let asset_class = match rng.below(100) {
        0..75 => AssetClass::Equity,
        75..90 => AssetClass::Bond,
        90..94 => AssetClass::Future,
        _ => AssetClass::Option,
    };
    let (country, region, currency, fx_rate_to_usd, _) = COUNTRIES[country_frequency.sample(rng)];
    let (sector, industries) = SECTORS[rng.index(SECTORS.len())];
    let industry = industries[rng.index(industries.len())];
    let root = unique_root(rng, used_tickers);
    let name = format!(
        "{} {} {}",
        NAME_WORDS[rng.index(NAME_WORDS.len())],
        NAME_WORDS[rng.index(NAME_WORDS.len())],
        NAME_SUFFIXES[rng.index(NAME_SUFFIXES.len())]
    );
    // Prices are typical USD prices converted to the local currency, so a Korean
    // stock trades in tens of thousands of won. Bonds are quoted per 100 of face
    // value in any currency.
    let fx = fx_rate_to_usd as f64 / 1e8;
    let mut security = Security {
        ticker: root.clone(),
        name,
        asset_class,
        sector: Some(sector),
        industry: Some(industry),
        country,
        region,
        currency,
        fx_rate_to_usd,
        base_price: 0.0,
        price_decimals: 2,
        multiplier: 100,
        beta: 1.0,
        volatility: 0.3,
        average_daily_volume: 0.0,
        bond: None,
        expiry: None,
    };
    match asset_class {
        AssetClass::Equity => {
            security.base_price = log_normal(rng, libm::log(60.0), 1.0).clamp(0.5, 5_000.0) / fx;
            security.price_decimals = if rng.chance(0.2) { 4 } else { 2 };
            security.beta = 1.0 + 0.35 * standard_normal(rng);
            security.volatility = log_normal(rng, libm::log(0.3), 0.35);
            security.average_daily_volume = log_normal(rng, libm::log(2.0e6), 1.2);
        }
        AssetClass::Bond => {
            let coupon_rate = (rng.uniform(0.5, 8.0) * 8.0).round() / 8.0;
            let years = 1 + rng.below(30) as i16;
            let maturity = date(reference.year() + years, 1 + rng.below(12) as i8, 15);
            let duration = f64::from(years) * rng.uniform(0.6, 0.9);
            let yield_to_maturity = (coupon_rate + 1.5 * standard_normal(rng)).max(0.05);
            security.ticker = format!("{root} {coupon_rate:.3} {}", maturity.year());
            security.base_price = rng.uniform(85.0, 110.0);
            security.price_decimals = 3;
            security.multiplier = 1;
            security.beta = 0.1 * standard_normal(rng);
            security.volatility = log_normal(rng, libm::log(0.06), 0.3);
            security.average_daily_volume = log_normal(rng, libm::log(5.0e6), 0.8);
            security.bond = Some(BondTerms {
                coupon_rate,
                maturity,
                duration,
                yield_to_maturity,
                rating: RATINGS[rating_frequency.sample(rng)].0,
            });
        }
        AssetClass::Future => {
            let expiry = quarterly_expiries_after(reference)[rng.index(4)];
            let code = FUTURE_MONTH_CODES[(expiry.month() / 3 - 1) as usize];
            security.ticker = format!("{root}{code}{}", expiry.year() % 10);
            security.sector = None;
            security.industry = None;
            security.base_price = log_normal(rng, libm::log(2_000.0), 1.0) / fx;
            security.multiplier = FUTURE_MULTIPLIERS[rng.index(FUTURE_MULTIPLIERS.len())];
            security.volatility = log_normal(rng, libm::log(0.18), 0.3);
            security.average_daily_volume = log_normal(rng, libm::log(1.0e5), 1.0);
            security.expiry = Some(expiry);
        }
        AssetClass::Option => {
            let expiry = reference
                .checked_add(jiff::Span::new().days(7 + rng.below(724) as i64))
                .expect("in range");
            let strike = (log_normal(rng, libm::log(100.0), 0.6) / 5.0).round() * 5.0;
            let kind = if rng.chance(0.5) { 'C' } else { 'P' };
            security.ticker = format!("{root} {expiry} {kind}{strike:.0}");
            security.base_price = log_normal(rng, libm::log(8.0), 1.0) / fx;
            security.multiplier = 10_000;
            security.beta = 1.0 + 0.5 * standard_normal(rng);
            security.volatility = log_normal(rng, libm::log(0.6), 0.3);
            security.average_daily_volume = log_normal(rng, libm::log(5.0e3), 1.0);
            security.expiry = Some(expiry);
        }
    }
    debug_assert!(security.base_price > 0.0);
    debug_assert!(from_f64(security.base_price, 6) > 0);
    security
}

/// Returns the next four quarterly expiries (the 18th of March, June, September and
/// December) strictly after `reference`.
fn quarterly_expiries_after(reference: Date) -> [Date; 4] {
    let mut expiries = [reference; 4];
    let mut year = reference.year();
    let mut month = 3 * ((reference.month() - 1) / 3 + 1);
    for expiry in &mut expiries {
        loop {
            let candidate = date(year, month, 18);
            if month == 12 {
                year += 1;
                month = 3;
            } else {
                month += 3;
            }
            if candidate > reference {
                *expiry = candidate;
                break;
            }
        }
    }
    expiries
}

/// Draws a ticker root of one to five letters that no earlier security uses.
fn unique_root(rng: &mut Rng, used: &mut HashSet<String>) -> String {
    loop {
        let len = match rng.below(100) {
            0..5 => 1,
            5..20 => 2,
            20..55 => 3,
            55..90 => 4,
            _ => 5,
        };
        let root: String = (0..len)
            .map(|_| char::from(b'A' + rng.below(26) as u8))
            .collect();
        if used.insert(root.clone()) {
            return root;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFERENCE: Date = date(2026, 10, 30);

    #[test]
    fn generated_tickers_never_collide_with_hostile_ones() {
        for seed in 0..20 {
            let u = Universe::new(seed, 10_000, true, REFERENCE).unwrap();
            let mut seen = HashSet::new();
            for s in &u.securities {
                assert!(
                    seen.insert(s.ticker.as_str()),
                    "seed {seed}: {:?} twice",
                    s.ticker
                );
            }
        }
    }

    #[test]
    fn maturities_and_expiries_follow_the_reference_date() {
        let u = Universe::new(9, 10_000, false, REFERENCE).unwrap();
        for s in &u.securities {
            if let Some(expiry) = s.expiry {
                assert!(expiry > REFERENCE, "{} expires {expiry}", s.ticker);
            }
            if let Some(bond) = &s.bond {
                assert!(
                    bond.maturity > REFERENCE,
                    "{} matures {}",
                    s.ticker,
                    bond.maturity
                );
            }
        }
        assert_eq!(
            quarterly_expiries_after(date(2026, 12, 18)),
            [
                date(2027, 3, 18),
                date(2027, 6, 18),
                date(2027, 9, 18),
                date(2027, 12, 18)
            ]
        );
        assert_eq!(
            quarterly_expiries_after(date(2026, 1, 2))[0],
            date(2026, 3, 18)
        );
    }

    #[test]
    fn derivative_prices_are_in_local_currency() {
        let u = Universe::new(10, 10_000, false, REFERENCE).unwrap();
        for s in &u.securities {
            let usd = s.base_price * s.fx_rate_to_usd as f64 / 1e8;
            if s.asset_class != AssetClass::Bond {
                assert!(usd > 0.0001 && usd < 1e7, "{} is worth {usd} USD", s.ticker);
            }
        }
    }

    #[test]
    fn rejects_an_empty_universe() {
        assert!(Universe::new(1, 0, false, REFERENCE).is_err());
    }

    #[test]
    fn is_reproducible() {
        let a = Universe::new(5, 500, false, REFERENCE).unwrap();
        let b = Universe::new(5, 500, false, REFERENCE).unwrap();
        let tickers = |u: &Universe| {
            u.securities
                .iter()
                .map(|s| s.ticker.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(tickers(&a), tickers(&b));
        assert_eq!(a.books.len(), b.books.len());
    }

    #[test]
    fn tickers_are_unique() {
        let u = Universe::new(6, 10_000, true, REFERENCE).unwrap();
        let distinct: HashSet<&str> = u.securities.iter().map(|s| s.ticker.as_str()).collect();
        assert_eq!(distinct.len(), u.securities.len());
    }

    #[test]
    fn hostile_universe_differs_only_in_tickers() {
        let calm = Universe::new(7, 2_000, false, REFERENCE).unwrap();
        let hostile = Universe::new(7, 2_000, true, REFERENCE).unwrap();
        let renamed = calm
            .securities
            .iter()
            .zip(&hostile.securities)
            .filter(|(a, b)| a.ticker != b.ticker)
            .count();
        assert_eq!(
            renamed,
            HOSTILE_RANKS.iter().filter(|&&r| r < 2_000).count()
        );
        for (a, b) in calm.securities.iter().zip(&hostile.securities) {
            assert_eq!(a.base_price.to_bits(), b.base_price.to_bits());
        }
        assert_eq!(hostile.securities[0].ticker, HOSTILE_TICKERS[0]);
    }

    #[test]
    fn asset_class_terms_are_consistent() {
        let u = Universe::new(8, 5_000, false, REFERENCE).unwrap();
        for s in &u.securities {
            assert!(s.base_price > 0.0);
            match s.asset_class {
                AssetClass::Equity => assert!(s.bond.is_none() && s.expiry.is_none()),
                AssetClass::Bond => assert!(s.bond.is_some() && s.multiplier == 1),
                AssetClass::Future => assert!(s.expiry.is_some() && s.sector.is_none()),
                AssetClass::Option => assert!(s.expiry.is_some() && s.multiplier == 10_000),
            }
        }
    }
}
