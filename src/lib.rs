use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use calamine::{Data, Reader, Xlsx, open_workbook, open_workbook_auto};
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QuarterHour {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SppRecord {
    pub timestamp: QuarterHour,
    pub weight: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BelpexRecord {
    pub timestamp: QuarterHour,
    pub euro_per_mwh: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PriceFormula {
    pub coefficient: Decimal,
    pub offset: Decimal,
}

impl PriceFormula {
    pub fn apply(self, belpex_price: Decimal) -> Decimal {
        self.coefficient * belpex_price + self.offset
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppConfig {
    pub files: Option<InputFiles>,
    pub prices: Vec<ConfiguredPrice>,
}

impl AppConfig {
    pub fn from_toml(input: &str) -> Result<Self> {
        let raw: RawAppConfig = toml::from_str(input).context("failed to parse pricing config")?;
        raw.try_into()
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            files: None,
            prices: vec![
                ConfiguredPrice {
                    name: "Injection".to_owned(),
                    index: WeightedIndexId::BelpexQSpp,
                    formula: PriceFormula {
                        coefficient: Decimal::new(95, 3),
                        offset: -Decimal::new(25, 1),
                    },
                    vat_percent: Decimal::new(6, 0),
                },
                ConfiguredPrice {
                    name: "Consumption Flanders".to_owned(),
                    index: WeightedIndexId::BelpexQRlpVl,
                    formula: PriceFormula {
                        coefficient: Decimal::new(107, 3),
                        offset: Decimal::new(15, 1),
                    },
                    vat_percent: Decimal::new(6, 0),
                },
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputFiles {
    pub spp: PathBuf,
    pub rlp: PathBuf,
    pub belpex: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfiguredPrice {
    pub name: String,
    pub index: WeightedIndexId,
    pub formula: PriceFormula,
    pub vat_percent: Decimal,
}

impl ConfiguredPrice {
    pub fn calculate(&self, weighted_index: &WeightedIndexResult) -> PriceCalculation {
        let price_excl_vat = self.formula.apply(weighted_index.weighted_price);

        PriceCalculation {
            name: self.name.clone(),
            index: self.index,
            formula: self.formula,
            vat_percent: self.vat_percent,
            price_excl_vat,
            price_incl_vat: price_incl_vat(price_excl_vat, self.vat_percent),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct RawAppConfig {
    files: Option<RawInputFiles>,
    prices: Vec<RawConfiguredPrice>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawInputFiles {
    spp: PathBuf,
    rlp: PathBuf,
    belpex: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
struct RawConfiguredPrice {
    name: String,
    index: String,
    #[serde(with = "rust_decimal::serde::str")]
    a: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    b: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    vat_percent: Decimal,
}

impl TryFrom<RawAppConfig> for AppConfig {
    type Error = anyhow::Error;

    fn try_from(raw: RawAppConfig) -> Result<Self> {
        if raw.prices.is_empty() {
            bail!("pricing config must define at least one [[prices]] entry");
        }

        let prices = raw
            .prices
            .into_iter()
            .map(|price| {
                Ok(ConfiguredPrice {
                    name: price.name,
                    index: price.index.parse()?,
                    formula: PriceFormula {
                        coefficient: price.a,
                        offset: price.b,
                    },
                    vat_percent: price.vat_percent,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Self {
            files: raw.files.map(|files| InputFiles {
                spp: files.spp,
                rlp: files.rlp,
                belpex: files.belpex,
            }),
            prices,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PricingConfig {
    pub export_formula: PriceFormula,
    pub import_formula: PriceFormula,
    pub vat_percent: Decimal,
}

impl Default for PricingConfig {
    fn default() -> Self {
        let default = AppConfig::default();
        Self {
            export_formula: default.prices[0].formula,
            import_formula: default.prices[1].formula,
            vat_percent: default.prices[0].vat_percent,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WeightedIndexId {
    BelpexHSpp,
    BelpexQSpp,
    BelpexHRlpVl,
    BelpexQRlpVl,
    BelpexHRlpBe,
    BelpexQRlpBe,
    BelpexHRlpM,
}

impl WeightedIndexId {
    pub const ALL: [Self; 7] = [
        Self::BelpexHSpp,
        Self::BelpexQSpp,
        Self::BelpexHRlpVl,
        Self::BelpexQRlpVl,
        Self::BelpexHRlpBe,
        Self::BelpexQRlpBe,
        Self::BelpexHRlpM,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::BelpexHSpp => "belpex-h-spp",
            Self::BelpexQSpp => "belpex-q-spp",
            Self::BelpexHRlpVl => "belpex-h-rlp-vl",
            Self::BelpexQRlpVl => "belpex-q-rlp-vl",
            Self::BelpexHRlpBe => "belpex-h-rlp-be",
            Self::BelpexQRlpBe => "belpex-q-rlp-be",
            Self::BelpexHRlpM => "belpex-h-rlp-m",
        }
    }
}

impl std::fmt::Display for WeightedIndexId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for WeightedIndexId {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "belpex-h-spp" => Ok(Self::BelpexHSpp),
            "belpex-q-spp" => Ok(Self::BelpexQSpp),
            "belpex-h-rlp-vl" => Ok(Self::BelpexHRlpVl),
            "belpex-q-rlp-vl" => Ok(Self::BelpexQRlpVl),
            "belpex-h-rlp-be" => Ok(Self::BelpexHRlpBe),
            "belpex-q-rlp-be" => Ok(Self::BelpexQRlpBe),
            "belpex-h-rlp-m" => Ok(Self::BelpexHRlpM),
            _ => bail!("unknown weighted index {value:?}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Hour {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Calculation {
    pub matched_quarter_hours: usize,
    pub total_spp_weight: Decimal,
    pub belpex_spp_be: Decimal,
    pub terugleveringsvergoeding: Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeightedIndexResult {
    pub id: WeightedIndexId,
    pub matched_quarter_hours: usize,
    pub total_weight: Decimal,
    pub weighted_price: Decimal,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PriceCalculation {
    pub name: String,
    pub index: WeightedIndexId,
    pub formula: PriceFormula,
    pub vat_percent: Decimal,
    pub price_excl_vat: Decimal,
    pub price_incl_vat: Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalculationComparison {
    pub hourly_averaged: Calculation,
    pub exact_quarter_hour: Calculation,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImportCalculation {
    pub matched_quarter_hours: usize,
    pub total_rlp_weight: Decimal,
    pub belpex_rlp_vl: Decimal,
    pub price_excl_vat: Decimal,
    pub price_incl_vat: Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImportCalculationComparison {
    pub hourly_averaged: ImportCalculation,
    pub exact_quarter_hour: ImportCalculation,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppCalculation {
    pub export: CalculationComparison,
    pub import: ImportCalculationComparison,
    pub import_belgium: ImportCalculationComparison,
    pub belpex_rlp_m: ImportCalculation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfiguredAppCalculation {
    pub indices: Vec<WeightedIndexResult>,
    pub prices: Vec<PriceCalculation>,
}

pub fn calculate_from_files(
    spp_path: &Path,
    rlp_path: &Path,
    belpex_path: &Path,
    year: u16,
    month: u8,
) -> Result<AppCalculation> {
    calculate_from_files_with_config(
        spp_path,
        rlp_path,
        belpex_path,
        year,
        month,
        PricingConfig::default(),
    )
}

pub fn calculate_configured_from_files(
    spp_path: &Path,
    rlp_path: &Path,
    belpex_path: &Path,
    year: u16,
    month: u8,
    config: &AppConfig,
) -> Result<ConfiguredAppCalculation> {
    if !(1..=12).contains(&month) {
        bail!("month must be in the range 1..=12, got {month}");
    }

    let spp_records = read_spp_records(spp_path, year, month)
        .with_context(|| format!("failed to read SPP workbook {}", spp_path.display()))?;
    let rlp_flanders_records = read_rlp_records(rlp_path, year, month, 7..=14, "VL RLP")
        .with_context(|| format!("failed to read RLP workbook {}", rlp_path.display()))?;
    let rlp_belgium_records = read_rlp_records(rlp_path, year, month, 7..=28, "BE RLP")
        .with_context(|| format!("failed to read RLP workbook {}", rlp_path.display()))?;
    let rlp_eneco_records = read_rlp_eneco_records(rlp_path, year, month)
        .with_context(|| format!("failed to read RLP workbook {}", rlp_path.display()))?;
    let belpex_records = read_belpex_records(belpex_path, year, month)
        .with_context(|| format!("failed to read Belpex workbook {}", belpex_path.display()))?;

    calculate_configured(
        &spp_records,
        &rlp_flanders_records,
        &rlp_belgium_records,
        &rlp_eneco_records,
        &belpex_records,
        config,
    )
}

pub fn calculate_configured(
    spp_records: &[SppRecord],
    rlp_flanders_records: &[SppRecord],
    rlp_belgium_records: &[SppRecord],
    rlp_eneco_records: &[SppRecord],
    belpex_records: &[BelpexRecord],
    config: &AppConfig,
) -> Result<ConfiguredAppCalculation> {
    let indices = calculate_weighted_indices(
        spp_records,
        rlp_flanders_records,
        rlp_belgium_records,
        rlp_eneco_records,
        belpex_records,
    )?;
    let by_id = indices
        .iter()
        .map(|index| (index.id, index))
        .collect::<HashMap<_, _>>();
    let prices = config
        .prices
        .iter()
        .map(|price| {
            let index = by_id
                .get(&price.index)
                .with_context(|| format!("missing weighted index {}", price.index))?;
            Ok(price.calculate(index))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(ConfiguredAppCalculation { indices, prices })
}

pub fn calculate_weighted_indices(
    spp_records: &[SppRecord],
    rlp_flanders_records: &[SppRecord],
    rlp_belgium_records: &[SppRecord],
    rlp_eneco_records: &[SppRecord],
    belpex_records: &[BelpexRecord],
) -> Result<Vec<WeightedIndexResult>> {
    let hourly_belpex_records = average_belpex_to_hourly(belpex_records);
    let inputs = [
        (
            WeightedIndexId::BelpexHSpp,
            spp_records,
            hourly_belpex_records.as_slice(),
        ),
        (WeightedIndexId::BelpexQSpp, spp_records, belpex_records),
        (
            WeightedIndexId::BelpexHRlpVl,
            rlp_flanders_records,
            hourly_belpex_records.as_slice(),
        ),
        (
            WeightedIndexId::BelpexQRlpVl,
            rlp_flanders_records,
            belpex_records,
        ),
        (
            WeightedIndexId::BelpexHRlpBe,
            rlp_belgium_records,
            hourly_belpex_records.as_slice(),
        ),
        (
            WeightedIndexId::BelpexQRlpBe,
            rlp_belgium_records,
            belpex_records,
        ),
        (
            WeightedIndexId::BelpexHRlpM,
            rlp_eneco_records,
            hourly_belpex_records.as_slice(),
        ),
    ];

    inputs
        .into_iter()
        .map(|(id, weights, prices)| {
            calculate_weighted_index(weights, prices).map(|mut result| {
                result.id = id;
                result
            })
        })
        .collect()
}

pub fn calculate_from_files_with_config(
    spp_path: &Path,
    rlp_path: &Path,
    belpex_path: &Path,
    year: u16,
    month: u8,
    config: PricingConfig,
) -> Result<AppCalculation> {
    if !(1..=12).contains(&month) {
        bail!("month must be in the range 1..=12, got {month}");
    }

    let spp_records = read_spp_records(spp_path, year, month)
        .with_context(|| format!("failed to read SPP workbook {}", spp_path.display()))?;
    let rlp_flanders_records = read_rlp_records(rlp_path, year, month, 7..=14, "VL RLP")
        .with_context(|| format!("failed to read RLP workbook {}", rlp_path.display()))?;
    let rlp_belgium_records = read_rlp_records(rlp_path, year, month, 7..=28, "BE RLP")
        .with_context(|| format!("failed to read RLP workbook {}", rlp_path.display()))?;
    let rlp_eneco_records = read_rlp_eneco_records(rlp_path, year, month)
        .with_context(|| format!("failed to read RLP workbook {}", rlp_path.display()))?;
    let belpex_records = read_belpex_records(belpex_path, year, month)
        .with_context(|| format!("failed to read Belpex workbook {}", belpex_path.display()))?;
    let hourly_belpex_records = average_belpex_to_hourly(&belpex_records);

    Ok(AppCalculation {
        export: calculate_both_with_config(&spp_records, &belpex_records, config)?,
        import: calculate_import_both_with_config(&rlp_flanders_records, &belpex_records, config)?,
        import_belgium: calculate_import_both_with_config(
            &rlp_belgium_records,
            &belpex_records,
            config,
        )?,
        belpex_rlp_m: calculate_import_with_config(
            &rlp_eneco_records,
            &hourly_belpex_records,
            config,
        )?,
    })
}

pub fn calculate_both(
    spp_records: &[SppRecord],
    belpex_records: &[BelpexRecord],
) -> Result<CalculationComparison> {
    calculate_both_with_config(spp_records, belpex_records, PricingConfig::default())
}

pub fn calculate_both_with_config(
    spp_records: &[SppRecord],
    belpex_records: &[BelpexRecord],
    config: PricingConfig,
) -> Result<CalculationComparison> {
    Ok(CalculationComparison {
        hourly_averaged: calculate_with_config(
            spp_records,
            &average_belpex_to_hourly(belpex_records),
            config,
        )?,
        exact_quarter_hour: calculate_with_config(spp_records, belpex_records, config)?,
    })
}

pub fn calculate(
    spp_records: &[SppRecord],
    belpex_records: &[BelpexRecord],
) -> Result<Calculation> {
    calculate_with_config(spp_records, belpex_records, PricingConfig::default())
}

pub fn calculate_with_config(
    spp_records: &[SppRecord],
    belpex_records: &[BelpexRecord],
    config: PricingConfig,
) -> Result<Calculation> {
    let weighted_index = calculate_weighted_index(spp_records, belpex_records)?;

    Ok(Calculation {
        matched_quarter_hours: weighted_index.matched_quarter_hours,
        total_spp_weight: weighted_index.total_weight,
        belpex_spp_be: weighted_index.weighted_price,
        terugleveringsvergoeding: terugleveringsvergoeding_with_config(
            weighted_index.weighted_price,
            config,
        ),
    })
}

fn calculate_weighted_index(
    spp_records: &[SppRecord],
    belpex_records: &[BelpexRecord],
) -> Result<WeightedIndexResult> {
    let belpex_by_timestamp: HashMap<QuarterHour, Decimal> = belpex_records
        .iter()
        .map(|record| {
            decimal_from_f64(record.euro_per_mwh)
                .map(|price| (record.timestamp, price))
                .with_context(|| format!("invalid Belpex price at {:?}", record.timestamp))
        })
        .collect::<Result<HashMap<_, _>>>()?;

    let mut matched_quarter_hours = 0usize;
    let mut total_spp_weight = Decimal::ZERO;
    let mut weighted_sum = Decimal::ZERO;

    for spp in spp_records {
        if let Some(price) = belpex_by_timestamp.get(&spp.timestamp) {
            let weight = decimal_from_f64(spp.weight)
                .with_context(|| format!("invalid SPP/RLP weight at {:?}", spp.timestamp))?;
            matched_quarter_hours += 1;
            total_spp_weight += weight;
            weighted_sum += weight * price;
        }
    }

    if matched_quarter_hours == 0 {
        bail!("no matching quarter-hour rows found between SPP and Belpex inputs");
    }
    if total_spp_weight == Decimal::ZERO {
        bail!("matched SPP rows have zero total weight");
    }

    let belpex_spp_be = weighted_sum / total_spp_weight;

    Ok(WeightedIndexResult {
        id: WeightedIndexId::BelpexQSpp,
        matched_quarter_hours,
        total_weight: total_spp_weight,
        weighted_price: belpex_spp_be,
    })
}

pub fn terugleveringsvergoeding(belpex_spp_be: Decimal) -> Decimal {
    terugleveringsvergoeding_with_config(belpex_spp_be, PricingConfig::default())
}

pub fn terugleveringsvergoeding_with_config(
    belpex_spp_be: Decimal,
    config: PricingConfig,
) -> Decimal {
    config.export_formula.apply(belpex_spp_be)
}

pub fn calculate_import_both(
    rlp_records: &[SppRecord],
    belpex_records: &[BelpexRecord],
) -> Result<ImportCalculationComparison> {
    calculate_import_both_with_config(rlp_records, belpex_records, PricingConfig::default())
}

pub fn calculate_import_both_with_config(
    rlp_records: &[SppRecord],
    belpex_records: &[BelpexRecord],
    config: PricingConfig,
) -> Result<ImportCalculationComparison> {
    Ok(ImportCalculationComparison {
        hourly_averaged: calculate_import_with_config(
            rlp_records,
            &average_belpex_to_hourly(belpex_records),
            config,
        )?,
        exact_quarter_hour: calculate_import_with_config(rlp_records, belpex_records, config)?,
    })
}

pub fn calculate_import(
    rlp_records: &[SppRecord],
    belpex_records: &[BelpexRecord],
) -> Result<ImportCalculation> {
    calculate_import_with_config(rlp_records, belpex_records, PricingConfig::default())
}

pub fn calculate_import_with_config(
    rlp_records: &[SppRecord],
    belpex_records: &[BelpexRecord],
    config: PricingConfig,
) -> Result<ImportCalculation> {
    let weighted_price = calculate_with_config(rlp_records, belpex_records, config)?;
    let price_excl_vat = import_price_excl_vat_with_config(weighted_price.belpex_spp_be, config);

    Ok(ImportCalculation {
        matched_quarter_hours: weighted_price.matched_quarter_hours,
        total_rlp_weight: weighted_price.total_spp_weight,
        belpex_rlp_vl: weighted_price.belpex_spp_be,
        price_excl_vat,
        price_incl_vat: price_incl_vat_with_config(price_excl_vat, config),
    })
}

pub fn import_price_excl_vat(belpex_rlp_vl: Decimal) -> Decimal {
    import_price_excl_vat_with_config(belpex_rlp_vl, PricingConfig::default())
}

pub fn import_price_excl_vat_with_config(belpex_rlp_vl: Decimal, config: PricingConfig) -> Decimal {
    config.import_formula.apply(belpex_rlp_vl)
}

pub fn import_price_incl_vat(belpex_rlp_vl: Decimal) -> Decimal {
    import_price_incl_vat_with_config(belpex_rlp_vl, PricingConfig::default())
}

pub fn import_price_incl_vat_with_config(belpex_rlp_vl: Decimal, config: PricingConfig) -> Decimal {
    price_incl_vat_with_config(
        import_price_excl_vat_with_config(belpex_rlp_vl, config),
        config,
    )
}

fn price_incl_vat_with_config(price_excl_vat: Decimal, config: PricingConfig) -> Decimal {
    price_incl_vat(price_excl_vat, config.vat_percent)
}

fn price_incl_vat(price_excl_vat: Decimal, vat_percent: Decimal) -> Decimal {
    price_excl_vat * (Decimal::ONE + vat_percent / Decimal::new(100, 0))
}

pub fn average_belpex_to_hourly(records: &[BelpexRecord]) -> Vec<BelpexRecord> {
    let mut by_hour: HashMap<Hour, (f64, usize)> = HashMap::new();

    for record in records {
        let hour = Hour {
            year: record.timestamp.year,
            month: record.timestamp.month,
            day: record.timestamp.day,
            hour: record.timestamp.hour,
        };
        let entry = by_hour.entry(hour).or_insert((0.0, 0));
        entry.0 += record.euro_per_mwh;
        entry.1 += 1;
    }

    let mut hourly_records = Vec::with_capacity(by_hour.len() * 4);
    for (hour, (sum, count)) in by_hour {
        let average = sum / count as f64;
        for minute in [0, 15, 30, 45] {
            hourly_records.push(BelpexRecord {
                timestamp: QuarterHour {
                    year: hour.year,
                    month: hour.month,
                    day: hour.day,
                    hour: hour.hour,
                    minute,
                },
                euro_per_mwh: average,
            });
        }
    }

    hourly_records
}

pub fn parse_belpex_time(value: &str) -> Result<(u8, u8)> {
    let (hour, minute) = value
        .trim()
        .split_once('u')
        .ok_or_else(|| anyhow!("expected Belpex time like 23u45, got {value:?}"))?;
    let hour: u8 = hour
        .parse()
        .with_context(|| format!("invalid Belpex hour in {value:?}"))?;
    let minute: u8 = minute
        .parse()
        .with_context(|| format!("invalid Belpex minute in {value:?}"))?;

    if hour > 23 || !matches!(minute, 0 | 15 | 30 | 45) {
        bail!("invalid quarter-hour Belpex time {value:?}");
    }

    Ok((hour, minute))
}

fn read_spp_records(path: &Path, year: u16, month: u8) -> Result<Vec<SppRecord>> {
    let mut workbook: Xlsx<_> = open_workbook(path)?;
    let range = workbook
        .worksheet_range("SPP_ex-ante_2026")
        .context("missing sheet SPP_ex-ante_2026")?;

    let mut rows = range.rows();
    let header = rows.next().context("SPP sheet is empty")?;
    let columns = Columns::from_header(
        header,
        &["Year", "Month", "Day", "Hour", "Min", "SPPExanteBE"],
    )?;

    let mut records = Vec::new();
    for (index, row) in rows.enumerate() {
        let row_number = index + 2;
        let row_year = cell_u16(row, columns.get("Year")?, "Year", row_number)?;
        let row_month = cell_u8(row, columns.get("Month")?, "Month", row_number)?;

        if row_year != year || row_month != month {
            continue;
        }

        records.push(SppRecord {
            timestamp: QuarterHour {
                year: row_year,
                month: row_month,
                day: cell_u8(row, columns.get("Day")?, "Day", row_number)?,
                hour: cell_u8(row, columns.get("Hour")?, "Hour", row_number)?,
                minute: cell_u8(row, columns.get("Min")?, "Min", row_number)?,
            },
            weight: cell_f64(row, columns.get("SPPExanteBE")?, "SPPExanteBE", row_number)?,
        });
    }

    Ok(records)
}

fn read_belpex_records(path: &Path, year: u16, month: u8) -> Result<Vec<BelpexRecord>> {
    let mut workbook: Xlsx<_> = open_workbook(path)?;
    let sheet_name = workbook
        .sheet_names()
        .first()
        .cloned()
        .context("Belpex workbook has no worksheets")?;
    let range = workbook
        .worksheet_range(&sheet_name)
        .with_context(|| format!("missing Belpex worksheet {sheet_name}"))?;

    let mut header_row = None;
    for (index, row) in range.rows().enumerate() {
        if row
            .iter()
            .any(|cell| cell_string(cell).as_deref() == Some("Date"))
        {
            header_row = Some((index, row));
            break;
        }
    }

    let (header_index, header) =
        header_row.context("Belpex header row with Date column not found")?;
    let columns = Columns::from_header(header, &["Date", "Time", "Euro"])?;

    let mut records = Vec::new();
    for (index, row) in range.rows().enumerate().skip(header_index + 1) {
        let row_number = index + 1;
        let date = cell_string_at(row, columns.get("Date")?, "Date", row_number)?;
        let Some((day, row_month, row_year)) = parse_belpex_date(&date)? else {
            continue;
        };

        if row_year != year || row_month != month {
            continue;
        }

        let time = cell_string_at(row, columns.get("Time")?, "Time", row_number)?;
        let (hour, minute) = parse_belpex_time(&time)?;

        records.push(BelpexRecord {
            timestamp: QuarterHour {
                year: row_year,
                month: row_month,
                day,
                hour,
                minute,
            },
            euro_per_mwh: cell_f64(row, columns.get("Euro")?, "Euro", row_number)?,
        });
    }

    Ok(records)
}

fn read_rlp_records(
    path: &Path,
    year: u16,
    month: u8,
    weight_columns: std::ops::RangeInclusive<usize>,
    weight_label: &str,
) -> Result<Vec<SppRecord>> {
    let mut workbook = open_workbook_auto(path)?;
    let range = workbook
        .worksheet_range("RLP96UbyDGO")
        .context("missing sheet RLP96UbyDGO")?;

    let header = range
        .rows()
        .nth(2)
        .context("RLP sheet is missing header row 3")?;
    let columns = Columns::from_header(header, &["Year", "Month", "Day", "h", "Min"])?;

    let mut records = Vec::new();
    for (index, row) in range.rows().enumerate().skip(3) {
        let row_number = index + 1;
        let row_year = cell_u16(row, columns.get("Year")?, "Year", row_number)?;
        let row_month = cell_u8(row, columns.get("Month")?, "Month", row_number)?;

        if row_year != year || row_month != month {
            continue;
        }

        records.push(SppRecord {
            timestamp: QuarterHour {
                year: row_year,
                month: row_month,
                day: cell_u8(row, columns.get("Day")?, "Day", row_number)?,
                hour: cell_u8(row, columns.get("h")?, "h", row_number)?,
                minute: cell_u8(row, columns.get("Min")?, "Min", row_number)?,
            },
            weight: average_cells(row, weight_columns.clone(), weight_label, row_number)?,
        });
    }

    Ok(records)
}

fn read_rlp_eneco_records(path: &Path, year: u16, month: u8) -> Result<Vec<SppRecord>> {
    let mut workbook = open_workbook_auto(path)?;
    let range = workbook
        .worksheet_range("RLP96UbyDGO")
        .context("missing sheet RLP96UbyDGO")?;

    let header = range
        .rows()
        .nth(2)
        .context("RLP sheet is missing header row 3")?;
    let columns = Columns::from_header(header, &["Year", "Month", "Day", "h", "Min"])?;

    let mut records = Vec::new();
    for (index, row) in range.rows().enumerate().skip(3) {
        let row_number = index + 1;
        let row_year = cell_u16(row, columns.get("Year")?, "Year", row_number)?;
        let row_month = cell_u8(row, columns.get("Month")?, "Month", row_number)?;

        if row_year != year || row_month != month {
            continue;
        }

        records.push(SppRecord {
            timestamp: QuarterHour {
                year: row_year,
                month: row_month,
                day: cell_u8(row, columns.get("Day")?, "Day", row_number)?,
                hour: cell_u8(row, columns.get("h")?, "h", row_number)?,
                minute: cell_u8(row, columns.get("Min")?, "Min", row_number)?,
            },
            weight: rlp_eneco_weight(row, row_number)?,
        });
    }

    Ok(records)
}

fn rlp_eneco_weight(row: &[Data], row_number: usize) -> Result<f64> {
    let fluvius = average_cells(row, 7..=14, "RLP Fluvius", row_number)?;
    let wallonia = average_cell_ranges(row, &[15..=23, 26..=28], "RLP Wallonia", row_number)?;
    let sibelga = average_cells(row, 24..=25, "RLP Sibelga", row_number)?;

    Ok((fluvius + wallonia + sibelga) / 3.0)
}

fn parse_belpex_date(value: &str) -> Result<Option<(u8, u8, u16)>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }

    let parts: Vec<_> = value.split('/').collect();
    if parts.len() != 3 {
        bail!("expected Belpex date like DD/MM/YYYY, got {value:?}");
    }

    let day: u8 = parts[0]
        .parse()
        .with_context(|| format!("invalid Belpex day in {value:?}"))?;
    let month: u8 = parts[1]
        .parse()
        .with_context(|| format!("invalid Belpex month in {value:?}"))?;
    let year: u16 = parts[2]
        .parse()
        .with_context(|| format!("invalid Belpex year in {value:?}"))?;

    Ok(Some((day, month, year)))
}

fn average_cells(
    row: &[Data],
    indices: std::ops::RangeInclusive<usize>,
    column: &str,
    row_number: usize,
) -> Result<f64> {
    average_cell_ranges(row, &[indices], column, row_number)
}

fn average_cell_ranges(
    row: &[Data],
    ranges: &[std::ops::RangeInclusive<usize>],
    column: &str,
    row_number: usize,
) -> Result<f64> {
    let mut sum = 0.0;
    let mut count = 0usize;

    for range in ranges {
        for index in range.clone() {
            sum += cell_f64(row, index, column, row_number)?;
            count += 1;
        }
    }

    if count == 0 {
        bail!("cannot average zero RLP columns at row {row_number}");
    }

    Ok(sum / count as f64)
}

struct Columns(HashMap<String, usize>);

impl Columns {
    fn from_header(header: &[Data], required: &[&str]) -> Result<Self> {
        let columns: HashMap<String, usize> = header
            .iter()
            .enumerate()
            .filter_map(|(index, cell)| cell_string(cell).map(|name| (name, index)))
            .collect();

        for name in required {
            if !columns.contains_key(*name) {
                bail!("missing required column {name}");
            }
        }

        Ok(Self(columns))
    }

    fn get(&self, name: &str) -> Result<usize> {
        self.0
            .get(name)
            .copied()
            .ok_or_else(|| anyhow!("missing required column {name}"))
    }
}

fn cell_string_at(row: &[Data], index: usize, column: &str, row_number: usize) -> Result<String> {
    row.get(index)
        .and_then(cell_string)
        .with_context(|| format!("missing or invalid {column} value at row {row_number}"))
}

fn cell_string(cell: &Data) -> Option<String> {
    match cell {
        Data::String(value) => Some(value.trim().to_owned()),
        Data::Float(value) => Some(format_float(*value)),
        Data::Int(value) => Some(value.to_string()),
        Data::DateTime(value) => Some(value.to_string()),
        Data::DateTimeIso(value) | Data::DurationIso(value) => Some(value.trim().to_owned()),
        Data::Bool(value) => Some(value.to_string()),
        Data::Empty | Data::Error(_) => None,
    }
}

fn cell_f64(row: &[Data], index: usize, column: &str, row_number: usize) -> Result<f64> {
    match row.get(index) {
        Some(Data::Float(value)) => Ok(*value),
        Some(Data::Int(value)) => Ok(*value as f64),
        Some(Data::String(value)) => value
            .trim()
            .parse()
            .with_context(|| format!("invalid {column} value at row {row_number}: {value:?}")),
        Some(other) => bail!("invalid {column} value at row {row_number}: {other:?}"),
        None => bail!("missing {column} value at row {row_number}"),
    }
}

fn cell_u16(row: &[Data], index: usize, column: &str, row_number: usize) -> Result<u16> {
    let value = cell_f64(row, index, column, row_number)?;
    if value.fract() != 0.0 || value < 0.0 || value > u16::MAX as f64 {
        bail!("invalid integer {column} value at row {row_number}: {value}");
    }
    Ok(value as u16)
}

fn cell_u8(row: &[Data], index: usize, column: &str, row_number: usize) -> Result<u8> {
    let value = cell_f64(row, index, column, row_number)?;
    if value.fract() != 0.0 || value < 0.0 || value > u8::MAX as f64 {
        bail!("invalid integer {column} value at row {row_number}: {value}");
    }
    Ok(value as u8)
}

fn format_float(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

fn decimal_from_f64(value: f64) -> Result<Decimal> {
    Decimal::from_f64(value).ok_or_else(|| anyhow!("cannot convert {value} to decimal"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(day: u8, hour: u8, minute: u8) -> QuarterHour {
        QuarterHour {
            year: 2026,
            month: 5,
            day,
            hour,
            minute,
        }
    }

    fn dec(value: i64, scale: u32) -> Decimal {
        Decimal::new(value, scale)
    }

    fn custom_config() -> PricingConfig {
        PricingConfig {
            export_formula: PriceFormula {
                coefficient: dec(2, 1),
                offset: -dec(1, 0),
            },
            import_formula: PriceFormula {
                coefficient: dec(5, 1),
                offset: dec(2, 0),
            },
            vat_percent: dec(21, 0),
        }
    }

    fn sample_belpex() -> [BelpexRecord; 2] {
        [
            BelpexRecord {
                timestamp: ts(1, 10, 0),
                euro_per_mwh: 30.0,
            },
            BelpexRecord {
                timestamp: ts(1, 10, 15),
                euro_per_mwh: 60.0,
            },
        ]
    }

    fn sample_weights() -> [SppRecord; 2] {
        [
            SppRecord {
                timestamp: ts(1, 10, 0),
                weight: 2.0,
            },
            SppRecord {
                timestamp: ts(1, 10, 15),
                weight: 1.0,
            },
        ]
    }

    fn index_by_id(indices: &[WeightedIndexResult], id: WeightedIndexId) -> WeightedIndexResult {
        *indices.iter().find(|index| index.id == id).unwrap()
    }

    #[test]
    fn parses_belpex_time() {
        assert_eq!(parse_belpex_time("23u45").unwrap(), (23, 45));
        assert_eq!(parse_belpex_time("0u00").unwrap(), (0, 0));
        assert!(parse_belpex_time("24u00").is_err());
        assert!(parse_belpex_time("12:15").is_err());
    }

    #[test]
    fn joins_on_quarter_hour_timestamp() {
        let spp = [
            SppRecord {
                timestamp: ts(1, 12, 0),
                weight: 1.0,
            },
            SppRecord {
                timestamp: ts(1, 12, 15),
                weight: 1.0,
            },
        ];
        let belpex = [BelpexRecord {
            timestamp: ts(1, 12, 15),
            euro_per_mwh: 40.0,
        }];

        let result = calculate(&spp, &belpex).unwrap();

        assert_eq!(result.matched_quarter_hours, 1);
        assert_eq!(result.belpex_spp_be, dec(40, 0));
    }

    #[test]
    fn calculates_weighted_average() {
        let spp = [
            SppRecord {
                timestamp: ts(1, 10, 0),
                weight: 2.0,
            },
            SppRecord {
                timestamp: ts(1, 10, 15),
                weight: 1.0,
            },
        ];
        let belpex = [
            BelpexRecord {
                timestamp: ts(1, 10, 0),
                euro_per_mwh: 30.0,
            },
            BelpexRecord {
                timestamp: ts(1, 10, 15),
                euro_per_mwh: 60.0,
            },
        ];

        let result = calculate(&spp, &belpex).unwrap();

        assert_eq!(result.matched_quarter_hours, 2);
        assert_eq!(result.total_spp_weight, dec(3, 0));
        assert_eq!(result.belpex_spp_be, dec(40, 0));
    }

    #[test]
    fn parses_pricing_config() {
        let config = AppConfig::from_toml(
            r#"
[files]
spp = "SPP_ex-ante_and_ex-post_2026.xlsx"
rlp = "RLP0N 2026 Electricity all DSOs.xlsb"
belpex = "quarter-hourly-spot-belpex--c--elexys.xlsx"

[[prices]]
name = "Injection"
index = "belpex-q-spp"
a = "0.095"
b = "-2.5"
vat_percent = "6"

[[prices]]
name = "Consumption Belgium Hourly"
index = "belpex-h-rlp-be"
a = "0.107"
b = "1.5"
vat_percent = "21"
"#,
        )
        .unwrap();

        assert_eq!(config.prices.len(), 2);
        assert_eq!(
            config.files.as_ref().unwrap().spp,
            PathBuf::from("SPP_ex-ante_and_ex-post_2026.xlsx")
        );
        assert_eq!(
            config.files.as_ref().unwrap().rlp,
            PathBuf::from("RLP0N 2026 Electricity all DSOs.xlsb")
        );
        assert_eq!(
            config.files.as_ref().unwrap().belpex,
            PathBuf::from("quarter-hourly-spot-belpex--c--elexys.xlsx")
        );
        assert_eq!(config.prices[0].name, "Injection");
        assert_eq!(config.prices[0].index, WeightedIndexId::BelpexQSpp);
        assert_eq!(config.prices[0].formula.coefficient, dec(95, 3));
        assert_eq!(config.prices[0].formula.offset, -dec(25, 1));
        assert_eq!(config.prices[1].index, WeightedIndexId::BelpexHRlpBe);
        assert_eq!(config.prices[1].vat_percent, dec(21, 0));
    }

    #[test]
    fn rejects_unknown_weighted_index() {
        let error = AppConfig::from_toml(
            r#"
[[prices]]
name = "Bad"
index = "belpex-q-unknown"
a = "1"
b = "0"
vat_percent = "6"
"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown weighted index"));
    }

    #[test]
    fn averages_belpex_quarter_hours_to_hourly_prices() {
        let belpex = [
            BelpexRecord {
                timestamp: ts(1, 10, 0),
                euro_per_mwh: 10.0,
            },
            BelpexRecord {
                timestamp: ts(1, 10, 15),
                euro_per_mwh: 20.0,
            },
            BelpexRecord {
                timestamp: ts(1, 10, 30),
                euro_per_mwh: 30.0,
            },
            BelpexRecord {
                timestamp: ts(1, 10, 45),
                euro_per_mwh: 40.0,
            },
        ];

        let hourly = average_belpex_to_hourly(&belpex);

        assert_eq!(hourly.len(), 4);
        assert!(hourly.iter().all(|record| record.euro_per_mwh == 25.0));
        assert!(
            hourly
                .iter()
                .any(|record| record.timestamp == ts(1, 10, 45))
        );
    }

    #[test]
    fn calculates_both_hourly_and_exact_quarter_hour_modes() {
        let spp = [
            SppRecord {
                timestamp: ts(1, 10, 0),
                weight: 2.0,
            },
            SppRecord {
                timestamp: ts(1, 10, 15),
                weight: 1.0,
            },
        ];
        let belpex = [
            BelpexRecord {
                timestamp: ts(1, 10, 0),
                euro_per_mwh: 30.0,
            },
            BelpexRecord {
                timestamp: ts(1, 10, 15),
                euro_per_mwh: 60.0,
            },
        ];

        let comparison = calculate_both(&spp, &belpex).unwrap();

        assert_eq!(comparison.exact_quarter_hour.belpex_spp_be, dec(40, 0));
        assert_eq!(comparison.hourly_averaged.belpex_spp_be, dec(45, 0));
    }

    #[test]
    fn averages_vl_rlp_columns() {
        let row = vec![
            Data::Empty,
            Data::Empty,
            Data::Empty,
            Data::Empty,
            Data::Empty,
            Data::Empty,
            Data::Empty,
            Data::Float(1.0),
            Data::Float(2.0),
            Data::Float(3.0),
            Data::Float(4.0),
            Data::Float(5.0),
            Data::Float(6.0),
            Data::Float(7.0),
            Data::Float(8.0),
        ];

        assert_eq!(average_cells(&row, 7..=14, "VL RLP", 1).unwrap(), 4.5);
    }

    #[test]
    fn averages_be_rlp_columns() {
        let row = (0..32)
            .map(|value| Data::Float(value as f64))
            .collect::<Vec<_>>();

        assert_eq!(average_cells(&row, 7..=28, "BE RLP", 1).unwrap(), 17.5);
    }

    #[test]
    fn averages_eneco_rlp_regions_equally() {
        let row = (0..32)
            .map(|value| Data::Float(value as f64))
            .collect::<Vec<_>>();

        let result = rlp_eneco_weight(&row, 1).unwrap();

        assert!((result - 18.666666666666668).abs() < 0.000001);
    }

    #[test]
    fn calculates_import_formulas() {
        assert_eq!(import_price_excl_vat(dec(4585, 2)), dec(640595, 5));
        assert_eq!(import_price_incl_vat(dec(4585, 2)), dec(6790307, 6));
    }

    #[test]
    fn calculates_configured_formula_and_per_formula_vat() {
        let price = ConfiguredPrice {
            name: "Custom".to_owned(),
            index: WeightedIndexId::BelpexQSpp,
            formula: PriceFormula {
                coefficient: dec(5, 1),
                offset: dec(2, 0),
            },
            vat_percent: dec(21, 0),
        };
        let index = WeightedIndexResult {
            id: WeightedIndexId::BelpexQSpp,
            matched_quarter_hours: 2,
            total_weight: dec(3, 0),
            weighted_price: dec(40, 0),
        };

        let result = price.calculate(&index);

        assert_eq!(result.price_excl_vat, dec(22, 0));
        assert_eq!(result.price_incl_vat, dec(2662, 2));
    }

    #[test]
    fn default_config_recreates_current_default_formula_outputs() {
        let config = AppConfig::default();
        let indices = [
            WeightedIndexResult {
                id: WeightedIndexId::BelpexQSpp,
                matched_quarter_hours: 2,
                total_weight: dec(3, 0),
                weighted_price: dec(4585, 2),
            },
            WeightedIndexResult {
                id: WeightedIndexId::BelpexQRlpVl,
                matched_quarter_hours: 2,
                total_weight: dec(3, 0),
                weighted_price: dec(4585, 2),
            },
        ];
        let by_id = indices
            .iter()
            .map(|index| (index.id, index))
            .collect::<HashMap<_, _>>();
        let prices = config
            .prices
            .iter()
            .map(|price| price.calculate(by_id.get(&price.index).unwrap()))
            .collect::<Vec<_>>();

        assert_eq!(prices[0].price_excl_vat, dec(185575, 5));
        assert_eq!(prices[1].price_excl_vat, dec(640595, 5));
        assert_eq!(prices[1].price_incl_vat, dec(6790307, 6));
    }

    #[test]
    fn calculates_weighted_index_registry() {
        let spp = sample_weights();
        let rlp_flanders = sample_weights();
        let rlp_belgium = sample_weights();
        let rlp_eneco = sample_weights();
        let belpex = sample_belpex();

        let indices =
            calculate_weighted_indices(&spp, &rlp_flanders, &rlp_belgium, &rlp_eneco, &belpex)
                .unwrap();

        assert_eq!(
            indices.iter().map(|index| index.id).collect::<Vec<_>>(),
            WeightedIndexId::ALL
        );
        assert_eq!(
            index_by_id(&indices, WeightedIndexId::BelpexQSpp).weighted_price,
            dec(40, 0)
        );
        assert_eq!(
            index_by_id(&indices, WeightedIndexId::BelpexHSpp).weighted_price,
            dec(45, 0)
        );
        assert_eq!(
            index_by_id(&indices, WeightedIndexId::BelpexQRlpVl).weighted_price,
            dec(40, 0)
        );
        assert_eq!(
            index_by_id(&indices, WeightedIndexId::BelpexHRlpM).weighted_price,
            dec(45, 0)
        );
    }

    #[test]
    fn calculates_custom_export_formula() {
        let spp = [SppRecord {
            timestamp: ts(1, 10, 0),
            weight: 1.0,
        }];
        let belpex = [BelpexRecord {
            timestamp: ts(1, 10, 0),
            euro_per_mwh: 40.0,
        }];

        let result = calculate_with_config(&spp, &belpex, custom_config()).unwrap();

        assert_eq!(result.belpex_spp_be, dec(40, 0));
        assert_eq!(result.terugleveringsvergoeding, dec(7, 0));
    }

    #[test]
    fn calculates_custom_import_formula_and_vat() {
        let rlp = [SppRecord {
            timestamp: ts(1, 10, 0),
            weight: 1.0,
        }];
        let belpex = [BelpexRecord {
            timestamp: ts(1, 10, 0),
            euro_per_mwh: 40.0,
        }];

        let result = calculate_import_with_config(&rlp, &belpex, custom_config()).unwrap();

        assert_eq!(result.belpex_rlp_vl, dec(40, 0));
        assert_eq!(result.price_excl_vat, dec(22, 0));
        assert_eq!(result.price_incl_vat, dec(2662, 2));
    }

    #[test]
    fn calculates_import_both_hourly_and_exact_quarter_hour_modes() {
        let rlp = [
            SppRecord {
                timestamp: ts(1, 10, 0),
                weight: 2.0,
            },
            SppRecord {
                timestamp: ts(1, 10, 15),
                weight: 1.0,
            },
        ];
        let belpex = [
            BelpexRecord {
                timestamp: ts(1, 10, 0),
                euro_per_mwh: 30.0,
            },
            BelpexRecord {
                timestamp: ts(1, 10, 15),
                euro_per_mwh: 60.0,
            },
        ];

        let comparison = calculate_import_both(&rlp, &belpex).unwrap();

        assert_eq!(comparison.exact_quarter_hour.belpex_rlp_vl, dec(40, 0));
        assert_eq!(comparison.hourly_averaged.belpex_rlp_vl, dec(45, 0));
        assert_eq!(comparison.exact_quarter_hour.price_excl_vat, dec(578, 2));
        assert_eq!(comparison.exact_quarter_hour.price_incl_vat, dec(61268, 4));
    }

    #[test]
    fn calculates_formula() {
        assert_eq!(terugleveringsvergoeding(dec(2917, 2)), dec(27115, 5));
    }

    #[test]
    fn errors_for_no_matches_or_zero_weight() {
        let spp = [SppRecord {
            timestamp: ts(1, 1, 0),
            weight: 0.0,
        }];
        let belpex = [BelpexRecord {
            timestamp: ts(1, 1, 0),
            euro_per_mwh: 10.0,
        }];

        assert!(calculate(&[], &belpex).is_err());
        assert!(calculate(&spp, &belpex).is_err());
    }
}
