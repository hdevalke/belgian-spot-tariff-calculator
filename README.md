# Belgian spot tariff calculator

Calculate month-to-date Belgian variable electricity tariffs that use Belpex day-ahead spot prices. The tool combines public Synergrid SPP/RLP profile files with an Elexys Belpex day-ahead export, calculates weighted indices, and applies configured tariff formulas.

## What you need

1. Download the Synergrid production and reference load profiles from:
   <https://www.synergrid.be/nl/documentencentrum/statistieken-gegevens/profielen-slp-spp-rlp>

   Use the SPP file for production profiles and the RLP file for reference load profiles for the year you want to calculate.

2. Download the quarter-hourly Belpex day-ahead spot prices from:
   <https://www.elexys.be/en/insights/quarter-hourly-belpex-day-ahead-spot-be?from=2026-01-01>

   Adjust the date on the Elexys page if you need another year or period.

3. Put the downloaded files in this project folder, or note their full paths.

## Configure input files

The easiest way is to edit `config.toml`:

```toml
[files]
spp = "SPP_ex-ante_and_ex-post_2026.xlsx"
rlp = "RLP0N 2026 Electricity all DSOs.xlsb"
belpex = "quarter-hourly-spot-belpex--c--elexys.xlsx"
```

Update these paths to match the files you downloaded.

You can also pass the paths directly on the command line:

```sh
cargo run -- \
  --year 2026 \
  --month 1 \
  --spp "SPP_ex-ante_and_ex-post_2026.xlsx" \
  --rlp "RLP0N 2026 Electricity all DSOs.xlsb" \
  --belpex "quarter-hourly-spot-belpex--c--elexys.xlsx"
```

## Configure prices

Price formulas are configured in `config.toml` as:

```toml
[[prices]]
name = "Trevion Flex Injectie kwartier"
index = "belpex-q-spp"
a = "0.095"
b = "-2.5"
vat_percent = "0"
```

The calculator applies this formula:

```text
price = a * index + b
```

The result is printed in `ct/kWh` and `EUR/kWh`, both excluding and including VAT when VAT is configured.

Available index names:

- `belpex-h-spp`
- `belpex-q-spp`
- `belpex-h-rlp-vl`
- `belpex-q-rlp-vl`
- `belpex-h-rlp-be`
- `belpex-q-rlp-be`
- `belpex-h-rlp-m`

## Run a calculation

For January 2026 using `config.toml`:

```sh
cargo run -- --year 2026 --month 1
```

For another month, change `--month`. For another year, download the matching Synergrid and Belpex files, update `config.toml`, and change `--year`.

## Output

The output contains:

- the calculated year and month
- all weighted Belpex indices in `EUR/MWh`
- each configured price formula
- the resulting price in `ct/kWh` and `EUR/kWh`

If the Belpex export does not contain the complete month, the output shows how many hours were matched. This is useful for month-to-date calculations before the month is complete.

Example output:

```text
$ cargo run -- --year 2026 --month 4
   Compiling belgian-spot-tariff-calculator v0.1.0 (/home/hannes/code/spp)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.79s
     Running `target/debug/belgian-spot-tariff-calculator --year 2026 --month 4`
Year/month: 2026-04

Weighted indices:
  Index                  EUR/MWh
  belpex-h-spp             29.17 EUR/MWh
  belpex-q-spp             27.95 EUR/MWh
  belpex-h-rlp-vl          85.59 EUR/MWh
  belpex-q-rlp-vl          85.80 EUR/MWh
  belpex-h-rlp-be          84.49 EUR/MWh
  belpex-q-rlp-be          84.69 EUR/MWh
  belpex-h-rlp-m           82.99 EUR/MWh

Prices:

Trevion Flex Injectie kwartier:
  Formula          0.095 * belpex-q-spp + -2.5
  Price            0.155357 ct/kWh   0.001554 EUR/kWh

Trevion Flex Injectie uur:
  Formula          0.095 * belpex-h-spp + -2.5
  Price            0.270730 ct/kWh   0.002707 EUR/kWh

Trevion Flex Afname kwartier:
  Formula          0.107 * belpex-q-rlp-vl + 1.5
  Excl. VAT        10.680870 ct/kWh   0.106809 EUR/kWh
  Incl. 6% VAT     11.321722 ct/kWh   0.113217 EUR/kWh

Trevion Flex Afname uur:
  Formula          0.107 * belpex-h-rlp-vl + 1.5
  Excl. VAT        10.658628 ct/kWh   0.106586 EUR/kWh
  Incl. 6% VAT     11.298146 ct/kWh   0.112981 EUR/kWh
```
