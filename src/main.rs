#![allow(unused_imports)]

extern crate serde;
extern crate serde_json;
extern crate serde_yaml;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate lazy_static;
extern crate sputnikvm;
extern crate sputnikvm_network_classic;
extern crate sputnikvm_stateful;
extern crate block;
extern crate trie;
extern crate rand;
extern crate sha3;
extern crate bigint;
extern crate rustc_hex;
extern crate clap;
extern crate ethabi;

use sha3::{Digest, Keccak256};
use bigint::{H256, U256, M256, Address, Gas};
use sputnikvm::{ValidTransaction, VM, SeqTransactionVM, HeaderParams, VMStatus};
use sputnikvm_network_classic::MainnetEIP160Patch;
use sputnikvm_stateful::{MemoryStateful, LiteralAccount};
use block::TransactionAction;
use trie::{Database, MemoryDatabase};
use std::collections::HashMap;
use std::str::FromStr;
use std::rc::Rc;
use std::fs;
use std::process::Command;
use rand::Rng;
use rustc_hex::{FromHex, ToHex};
use sputnikvm::{
    Storage,
    AccountChange,
};
use clap::{App, Arg};
use std::io::Read;
use ethabi::{
    token::{
        Token,
        Tokenizer,
        StrictTokenizer,
        LenientTokenizer
    },
};

fn load_yaml(path: &str) -> serde_yaml::Value {
    let mut content = String::new();
    let mut file = fs::File::open(path).unwrap();
    file.read_to_string(&mut content).unwrap();
    serde_yaml::from_str(&content).unwrap()
}

struct Contract {
    /// Contract name
    name: String,
    /// Source code file path
    path: Option<String>,
    /// Contract address
    address: Option<Address>,
    /// Contract bytecode (solc --bin)
    bytecode: Vec<u8>,
    // /// Contract bytecode runtime (solc --bin-runtime)
    // bytecode_runtime: Vec<u8>,
    /// ethabi Contract
    abi: ethabi::Contract,
    // /// userdoc
    // userdoc: HashMap<String, String>,
}

impl Contract {
    /// load from path
    pub fn load(path: &str, name: &str) -> Contract {
        let output = Command::new("solc")
            .args(&["--combined-json", "abi,bin,bin-runtime,userdoc", path])
            .output()
            .expect("Failed call solc");
        let data: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let contract = data["contracts"]
            .as_object()
            .unwrap()
            .into_iter()
            .find(|(long_name, _)| long_name.ends_with(format!(":{}", name).as_str()))
            .map(|(_, contract)| contract.as_object().unwrap())
            .unwrap();

        let bytecode: Vec<u8> = contract["bin"]
            .as_str()
            .unwrap()
            .from_hex()
            .unwrap();
        let abi_str = contract["abi"].as_str().unwrap();
        let abi = ethabi::Contract::load(abi_str.as_bytes()).unwrap();
        Contract {
            bytecode,
            abi,
            name: name.to_string(),
            path: Some(path.to_string()),
            address: None,
        }
    }

    /// code + constructor-params
    pub fn data(&self, params: &[&str]) -> Vec<u8> {
        if let Some(ref constructor) = self.abi.constructor {
            let tokens: Vec<Token> = params
                .iter()
                .enumerate()
                .map(|(i, param_value)| {
                    let param_type = &constructor.inputs[i].kind;
                    LenientTokenizer::tokenize(param_type, param_value).unwrap()
                })
                .collect();
            constructor.encode_input(self.bytecode.clone(), &tokens).unwrap()
        } else {
            self.bytecode.clone()
        }
    }
}

#[derive(Debug)]
struct AccountInfo {
    pub nonce: U256,
    pub address: Address,
    pub balance: U256,
    pub storage: Storage,
    pub code: Rc<Vec<u8>>,
}

fn gen_account(data: Rc<Vec<u8>>) -> Option<AccountInfo> {
    let database = MemoryDatabase::default();
    let mut stateful = MemoryStateful::empty(&database);

    let vm: SeqTransactionVM<MainnetEIP160Patch> = stateful.execute(ValidTransaction {
        caller: None,
        gas_price: Gas::zero(),
        gas_limit: Gas::from(10000000000000u64),
        action: TransactionAction::Create,
        value: U256::zero(),
        input: data.clone(),
        nonce: U256::one(),
    }, HeaderParams {
        beneficiary: Address::default(),
        timestamp: 0,
        number: U256::zero(),
        difficulty: U256::zero(),
        gas_limit: Gas::max_value()
    }, &[]);
    match vm.status() {
        VMStatus::ExitedOk => (),
        err => panic!("error: {:?}", err),
    }

    for account in vm.current_state().unwrap().account_state.accounts() {
        if let AccountChange::Create {
            nonce, address, balance, storage, code
        } = account.clone()
        {
            return Some(AccountInfo {nonce, address, balance, storage, code});
        }
    }
    None
}

fn main() {
    let matches = App::new("create-genesis")
        .arg(
            Arg::with_name("contracts")
                .short("c")
                .long("contracts")
                .takes_value(true)
                .required(true)
                .help("Contracts YAML config path")
        )
        .arg(
            Arg::with_name("data")
                .short("d")
                .long("data")
                .takes_value(true)
                .required(true)
                .help("Initialize data(for solidity constructor arguments) YAML config path")
        )
        .get_matches();

    let contracts = load_yaml(matches.value_of("contracts").unwrap());
    let data = load_yaml(matches.value_of("data").unwrap());


    let code_str = "608060405234801561001057600080fd5b5060646000819055507f8fb1356be6b2a4e49ee94447eb9dcb8783f51c41dcddfe7919f945017d163bf3336064604051808373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020018281526020019250505060405180910390a161018a806100946000396000f30060806040526004361061004c576000357c0100000000000000000000000000000000000000000000000000000000900463ffffffff16806360fe47b1146100515780636d4ce63c1461007e575b600080fd5b34801561005d57600080fd5b5061007c600480360381019080803590602001909291905050506100a9565b005b34801561008a57600080fd5b50610093610155565b6040518082815260200191505060405180910390f35b7fc6d8c0af6d21f291e7c359603aa97e0ed500f04db6e983b9fce75a91c6b8da6b816040518082815260200191505060405180910390a1806000819055507ffd28ec3ec2555238d8ad6f9faf3e4cd10e574ce7e7ef28b73caa53f9512f65b93382604051808373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020018281526020019250505060405180910390a150565b600080549050905600a165627a7a723058201a1bf0066db9d92d10c43c76eb864787182721a9dcec35b3782d7a8e152c7a850029";
    let code_bytes: Rc<Vec<u8>> = Rc::new(code_str.from_hex().unwrap());

    let info = gen_account(code_bytes).unwrap();
    println!("account info={:?}", info);
}
