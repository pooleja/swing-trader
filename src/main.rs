use curl::easy::Easy;
use secp256k1::SecretKey;
use std::time::{SystemTime, UNIX_EPOCH};
extern crate curl;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
use hex_literal::hex;
// use secp256k1::SecretKey;
use chrono;
use std::str::FromStr;
use web3::{
    contract::{Contract, Options},
    signing::SecretKeyRef,
    types::{Address, U256},
};

#[derive(Debug, Serialize, Deserialize)]
struct Ohlc {
    high: String,
    timestamp: String,
    volume: String,
    low: String,
    close: String,
    open: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct OhlcData {
    pair: String,
    ohlc: Vec<Ohlc>,
}

#[derive(Debug, Serialize, Deserialize)]
struct BitstampPriceData {
    data: OhlcData,
}

#[derive(Debug, Serialize, Deserialize)]
struct BitstampCurrentPriceData {
    last: String,
}

#[tokio::main]
async fn main() -> web3::contract::Result<()> {
    ///////////////////////
    //
    // Get the simple moving average from the last 20 days
    //
    ///////////////////////
    let day_seconds = 24 * 60 * 60;
    let now = SystemTime::now();
    let since_the_epoch = now.duration_since(UNIX_EPOCH);
    let now_seconds = since_the_epoch.unwrap().as_secs();
    let twenty_days_ago = now_seconds - (20 * day_seconds);

    println!("Current Time {:?}", chrono::offset::Local::now());

    // Build the URL
    let url = format!(
        "https://www.bitstamp.net/api/v2/ohlc/ethusd/?step=86400&limit=365&start={}",
        twenty_days_ago
    );

    let mut json = Vec::new();

    let mut easy = Easy::new();
    easy.url(&url).unwrap();
    {
        let mut transfer = easy.transfer();
        transfer
            .write_function(|data| {
                json.extend_from_slice(data);
                Ok(data.len())
            })
            .unwrap();
        transfer.perform().unwrap();
    }

    let price_data: BitstampPriceData = serde_json::from_slice(&json).unwrap();
    // println!("{:#?}", price_data);

    println!("Length: {:#?}", price_data.data.ohlc.len());

    let mut total = 0.0;
    for data in &price_data.data.ohlc {
        total += data.close.parse::<f32>().unwrap();
    }

    let length = price_data.data.ohlc.len() as f32;

    let average = total / length;

    println!("Average: {:#?}", average);

    ///////////////////////
    //
    // Get the current price
    //
    ///////////////////////
    let current_url = "https://www.bitstamp.net/api/v2/ticker/ethusd";
    json = Vec::new();
    easy = Easy::new();
    easy.url(&current_url).unwrap();
    {
        let mut transfer = easy.transfer();
        transfer
            .write_function(|data| {
                json.extend_from_slice(data);
                Ok(data.len())
            })
            .unwrap();
        transfer.perform().unwrap();
    }

    let current_price_data: BitstampCurrentPriceData = serde_json::from_slice(&json).unwrap();

    println!("Last: {:#?}", current_price_data.last);

    ///////////////////////
    //
    // Init web3, accounts, contracts
    //
    ///////////////////////
    let weth_address = "0x9c3C9283D3e44854697Cd22D3Faa240Cfb032889"; // matic on mumbai
    let usdt_address = "0x3813e82e6f7098b9583FC0F33a962D02018B6803"; // usdt on mumbai
    let router_address = "0x8954AfA98594b838bda56FE4C12a09D7739D179b";

    let http = web3::transports::Http::new(
        "https://polygon-mumbai.infura.io/v3/35d10ca098794633a00978e9dc4bd2b3",
    )
    .unwrap();
    let web3 = web3::Web3::new(http);

    let weth_contract = Contract::from_json(
        web3.eth(),
        Address::from_str(weth_address).unwrap(),
        include_bytes!("./token.json"),
    )?;
    let usdt_contract = Contract::from_json(
        web3.eth(),
        Address::from_str(usdt_address).unwrap(),
        include_bytes!("./token.json"),
    )?;

    let my_account: Address = hex!("XXX").into();

    // Get the balances of each token
    let weth_result =
        weth_contract.query("balanceOf", (my_account,), None, Options::default(), None);
    let usdt_result =
        usdt_contract.query("balanceOf", (my_account,), None, Options::default(), None);

    let weth_balance_of: U256 = weth_result.await?;
    let usdt_balance_of: U256 = usdt_result.await?;

    println!("WETH Balance {}", weth_balance_of);
    println!("USDT Balance {}", usdt_balance_of);

    // Query decimals on tokens
    let weth_decimal_result = weth_contract.query("decimals", (), None, Options::default(), None);
    let usdt_decimal_result = usdt_contract.query("decimals", (), None, Options::default(), None);

    // Figure out decimals of each contract for future math
    let weth_decimals: U256 = weth_decimal_result.await?;
    let usdt_decimals: U256 = usdt_decimal_result.await?;
    let base: U256 = U256::from(10i32);
    let weth_pow = base.pow(weth_decimals);
    let usdt_pow = base.pow(usdt_decimals);

    let amount_in: U256;
    let mut path: Vec<Address> = Vec::new();
    let min_amount_out: U256;

    // Build current price and round
    let price_str_rounded = format!("{:.0}", f64::from_str(&current_price_data.last).unwrap());
    println!("price_str_rounded {}", price_str_rounded);
    let price_i32 = price_str_rounded.parse::<i32>().unwrap();
    let price = U256::from(price_i32);
    println!("Rounded Price {}", price);

    // Check which token has a balance
    if weth_balance_of > usdt_balance_of {
        // If price is greater than average, do not trade
        if (price_i32 as f32) > average {
            println!("Currently in WETH and price is greater than average... not trading");
            return Ok(());
        }
        println!("Currently in WETH, converting to USDT");

        // Swap path from weth to usdt
        path.push(Address::from_str(&weth_address).unwrap());
        path.push(Address::from_str(&usdt_address).unwrap());

        // Amount of weth to trade
        amount_in = weth_balance_of;
        // WETH balance times price times .95
        min_amount_out =
            weth_balance_of * price * usdt_pow * U256::from(95i32) / weth_pow / U256::from(100i32);
    } else {
        // If price is less than average, do not trade
        if (price_i32 as f32) < average {
            println!("Currently in USDT and price is lower than average... not trading");
            return Ok(());
        }
        println!("Currently in USDT, converting to WETH");

        // Swap from usdt to weth
        path.push(Address::from_str(&usdt_address).unwrap());
        path.push(Address::from_str(&weth_address).unwrap());

        // Amount of usdt to trade
        amount_in = usdt_balance_of;
        // USDT amount divided by price times .95
        min_amount_out =
            usdt_balance_of * weth_pow * U256::from(95i32) / price / usdt_pow / U256::from(100i32);
    }

    println!("Min Amount Out {}", min_amount_out);

    /////////////////////////////
    // Swap
    ////////////////////////////

    let router_contract = Contract::from_json(
        web3.eth(),
        Address::from_str(router_address).unwrap(),
        include_bytes!("./router.json"),
    )?;

    // Private key for address
    let private_key = SecretKey::from_str("XXXXXXXXXXX").unwrap();
    let sk = SecretKeyRef::new(&private_key);

    println!("amount_in {}", amount_in);

    let one_day_from_now = now_seconds + day_seconds;
    let tx = router_contract
        .signed_call(
            "swapExactTokensForTokens",
            (
                amount_in,                    // Amount of tokens to send in
                min_amount_out,               // Min amount to get out - 5% slippage
                path,       // Token list to trade through, WETH to USDT or USDT to WETH
                my_account, // Where traded tokens will end up
                U256::from(one_day_from_now), // Expiration one day from now
            ),
            Options::with(|opt| opt.gas = Some(300_000.into())), // use defaults except for gas limit
            sk,                                                  // Secret key for signing
        )
        .await?;

    println!("{:#?}", tx);

    Ok(())
}
