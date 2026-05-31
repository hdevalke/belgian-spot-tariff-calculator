use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use rust_decimal::Decimal;

#[derive(Debug, Parser)]
#[command(
    name = "belgian-spot-tariff-calculator",
    about = "Calculate Belgian variable tariffs from Synergrid profiles and Belpex day-ahead spot prices"
)]
struct Args {
    #[arg(long, value_name = "PATH")]
    spp: Option<PathBuf>,

    #[arg(long, value_name = "PATH")]
    rlp: Option<PathBuf>,

    #[arg(long, value_name = "PATH")]
    belpex: Option<PathBuf>,

    #[arg(long)]
    year: u16,

    #[arg(long)]
    month: u8,

    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config_path = args.config.or_else(|| {
        let default_path = PathBuf::from("config.toml");
        default_path.exists().then_some(default_path)
    });
    let config = load_config(config_path.as_ref())?;
    let spp = resolve_path(
        "spp",
        args.spp.as_ref(),
        config.files.as_ref().map(|files| &files.spp),
    )?;
    let rlp = resolve_path(
        "rlp",
        args.rlp.as_ref(),
        config.files.as_ref().map(|files| &files.rlp),
    )?;
    let belpex = resolve_path(
        "belpex",
        args.belpex.as_ref(),
        config.files.as_ref().map(|files| &files.belpex),
    )?;
    let calculation = belgian_spot_tariff_calculator::calculate_configured_from_files(
        spp, rlp, belpex, args.year, args.month, &config,
    )?;

    print_month(args.year, args.month, &calculation.indices);

    println!();
    println!("Weighted indices:");
    print_weighted_indices(&calculation.indices);

    println!();
    println!("Prices:");
    for price in &calculation.prices {
        print_price(price);
    }

    Ok(())
}

fn resolve_path<'a>(
    name: &str,
    cli_path: Option<&'a PathBuf>,
    config_path: Option<&'a PathBuf>,
) -> Result<&'a PathBuf> {
    cli_path
        .or(config_path)
        .ok_or_else(|| anyhow!("missing --{name} path; provide it on the CLI or in [files].{name}"))
}

fn load_config(path: Option<&PathBuf>) -> Result<belgian_spot_tariff_calculator::AppConfig> {
    let Some(path) = path else {
        return Ok(belgian_spot_tariff_calculator::AppConfig::default());
    };

    let input = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read pricing config {}", path.display()))?;
    belgian_spot_tariff_calculator::AppConfig::from_toml(&input)
        .with_context(|| format!("invalid pricing config {}", path.display()))
}

fn print_month(
    year: u16,
    month: u8,
    indices: &[belgian_spot_tariff_calculator::WeightedIndexResult],
) {
    let expected_hours = days_in_month(year, month).map(|days| days as usize * 24);
    let matched_hours = indices
        .iter()
        .map(|index| index.matched_quarter_hours / 4)
        .min();

    match (matched_hours, expected_hours) {
        (Some(matched), Some(expected)) if matched < expected => {
            println!(
                "Year/month: {:04}-{:02} (matched hours: {matched}/{expected})",
                year, month
            );
        }
        _ => println!("Year/month: {:04}-{:02}", year, month),
    }
}

fn print_weighted_indices(indices: &[belgian_spot_tariff_calculator::WeightedIndexResult]) {
    println!("  {:<18}  {:>10}", "Index", "EUR/MWh");
    for index in indices {
        let id = index.id.to_string();
        let price = rounded(index.weighted_price, 2).to_string();
        println!("  {id:<18}  {price:>10} EUR/MWh");
    }
}

fn print_price(price: &belgian_spot_tariff_calculator::PriceCalculation) {
    println!();
    println!("{}:", price.name);
    let price_excl_vat = rounded(price.price_excl_vat, 6);
    let price_excl_vat_eur = rounded(price.price_excl_vat / Decimal::new(100, 0), 6);
    let price_incl_vat = rounded(price.price_incl_vat, 6);
    let price_incl_vat_eur = rounded(price.price_incl_vat / Decimal::new(100, 0), 6);

    println!(
        "  {:<16} {} * {} + {}",
        "Formula",
        price.formula.coefficient.normalize(),
        price.index,
        price.formula.offset.normalize()
    );
    if price.vat_percent == Decimal::ZERO {
        print_price_line("Price", price_excl_vat, price_excl_vat_eur);
    } else {
        print_price_line("Excl. VAT", price_excl_vat, price_excl_vat_eur);
        print_price_line(
            &format!("Incl. {}% VAT", price.vat_percent.normalize()),
            price_incl_vat,
            price_incl_vat_eur,
        );
    }
}

fn print_price_line(label: &str, ct_per_kwh: Decimal, eur_per_kwh: Decimal) {
    println!(
        "  {:<16} {} ct/kWh   {} EUR/kWh",
        label, ct_per_kwh, eur_per_kwh
    );
}

fn days_in_month(year: u16, month: u8) -> Option<u8> {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => Some(31),
        4 | 6 | 9 | 11 => Some(30),
        2 if is_leap_year(year) => Some(29),
        2 => Some(28),
        _ => None,
    }
}

fn is_leap_year(year: u16) -> bool {
    let year = year as u32;
    year.is_multiple_of(4) && !year.is_multiple_of(100) || year.is_multiple_of(400)
}

fn rounded(value: Decimal, scale: u32) -> Decimal {
    value.round_dp(scale)
}
