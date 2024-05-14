use std::{path::PathBuf, io::Read, ops::Add, str::FromStr};

use anyhow::{anyhow, Context, Result};
use futures::{SinkExt, StreamExt, TryStreamExt};
use secp256k1::rand::AsByteSliceMut;
use structopt::StructOpt;

use std::vec::Vec;

use curv::arithmetic::Converter;
use curv::BigInt;

use multi_party_ecdsa::protocols::multi_party_ecdsa::gg_2020::state_machine::sign::{
    OfflineStage, SignManual,
};
use round_based::async_runtime::AsyncProtocol;
use round_based::Msg;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use reqwest;

use web3::{
    Web3,
    types::{
      H256, U256, Address, TransactionParameters, 
      Bytes, TransactionReceipt
    },
    transports::Http
};

use web3::signing::keccak256;

use rlp::RlpStream;

mod gg20_sm_client;
use gg20_sm_client::join_computation;

#[derive(Debug, StructOpt)]
struct Cli {
    #[structopt(short, long, default_value = "http://localhost:8000/")]
    address: surf::Url,
    #[structopt(short, long, default_value = "default-signing")]
    room: String,
    #[structopt(short, long)]
    local_share: PathBuf,

    #[structopt(short, long, use_delimiter(true))]
    parties: Vec<u16>,
    #[structopt(short, long)]
    data_to_sign: String
}

#[derive(Debug, StructOpt, Serialize, Deserialize)]
struct RawTx {
    nonce: U256,
    gasPrice: U256,
    gasLimit: U256,
    to: Address,
    value: U256,
    data: String
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Cli = Cli::from_args();
    let local_share = tokio::fs::read(args.local_share)
        .await
        .context("cannot read local share")?;
    let local_share = serde_json::from_slice(&local_share).context("parse local share")?;
    let number_of_parties = args.parties.len();

    let (i, incoming, outgoing) =
        join_computation(args.address.clone(), &format!("{}-offline", args.room))
            .await
            .context("join offline computation")?;

    let incoming = incoming.fuse();
    tokio::pin!(incoming);
    tokio::pin!(outgoing);

    let tx = RawTx {
        nonce: U256::from(1),
        to: Address::from_str("0x4C34dDDEeb110852b7D927F3e491f514fE70E022")?,
        gasLimit: U256::from_dec_str("1000000")?,
        gasPrice: U256::from_dec_str("500000000")?,
        value: U256::from_dec_str("1000000")?,
        data: String::from("")
    };
    let address = String::from("0x370f29c01c003751cbba2b8ce8c164c3df5b9acb");

    let mut rlp = RlpStream::new();
    let tmpV: i128 = 5;
    rlp.begin_list(9);
    rlp.append(&tx.nonce);
    rlp.append(&tx.gasPrice);
    rlp.append(&tx.gasLimit);
    rlp.append(&tx.to);
    rlp.append(&tx.value);
    rlp.append(&tx.data);
    rlp.append(&U256::from(tmpV));
    rlp.append(&0u8);
    rlp.append(&0u8);

    let raw = rlp.as_raw();
    let hash = keccak256(raw);

    println!("hash: {:?}", hash);

    let signing = OfflineStage::new(i, args.parties, local_share)?;

    let client = reqwest::Client::new();

    let mut map = HashMap::new();
    map.insert("data_to_sign", hash.clone());

    let uri = format!("http://localhost:8001/sign/{}", args.room);
    println!("Sending sign request to {:?}", uri);
    let res = client
        .post(&uri)
        .json(&map)
        .send();
    
    println!("starting online signing");

    let completed_offline_stage = AsyncProtocol::new(signing, incoming, outgoing)
        .run()
        .await
        .map_err(|e| anyhow!("protocol execution terminated with error: {}", e))?;

    let (_i, incoming, outgoing) = join_computation(args.address, &format!("{}-online", args.room))
        .await
        .context("join online computation")?;

    tokio::pin!(incoming);
    tokio::pin!(outgoing);

    let (signing, partial_signature) = SignManual::new(
        BigInt::from_bytes(&hash.to_vec()),
        completed_offline_stage,
    )?;

    outgoing
        .send(Msg {
            sender: i,
            receiver: None,
            body: partial_signature,
        })
        .await?;

    let partial_signatures: Vec<_> = incoming
        .take(number_of_parties - 1)
        .map_ok(|msg| msg.body)
        .try_collect()
        .await?;
    let signature = signing
        .complete(&partial_signatures)
        .context("online stage failed")?;

    let r = signature.r.to_bytes();
    let s = signature.s.to_bytes();
    let v_with_replay_protec = signature.recid as u64 + 35 + 5 * 2;

    rlp.clear();
    rlp.begin_list(9);
    rlp.append(&tx.nonce);
    rlp.append(&tx.gasPrice);
    rlp.append(&tx.gasLimit);
    rlp.append(&tx.to);
    rlp.append(&tx.value);
    rlp.append(&tx.data);
    rlp.append(&v_with_replay_protec);
    rlp.append(&U256::from_big_endian(&r));
    rlp.append(&U256::from_big_endian(&s));


    let raw_tx: Bytes = rlp.out().to_vec().into();
    println!("raw: {:?}", raw_tx);

    let transport = Http::new("https://goerli.infura.io/v3/c70df41dde354d22a1aea5b20aa930e7")?;
    let web3 = Web3::new(transport.clone());

    let res = web3.eth().send_raw_transaction(raw_tx).await;

    match res {
        Ok(v) => println!("tx confirmed: {}", v),
        Err(v) => println!("tx failed: {}", v.to_string())
    };
    
    // let signature = serde_json::to_string(&signature).context("serialize signature")?;
    // println!("{}", signature);
    Ok(())
}
