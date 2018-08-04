#![allow(unused_imports)]

extern crate serde;
extern crate serde_json;
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
use rand::Rng;
use rustc_hex::{FromHex, ToHex};
use sputnikvm::AccountChange;

fn main() {
    let database = MemoryDatabase::default();
    let mut stateful = MemoryStateful::empty(&database);

    let code_str = "608060405234801561001057600080fd5b5060646000819055507f8fb1356be6b2a4e49ee94447eb9dcb8783f51c41dcddfe7919f945017d163bf3336064604051808373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020018281526020019250505060405180910390a161018a806100946000396000f30060806040526004361061004c576000357c0100000000000000000000000000000000000000000000000000000000900463ffffffff16806360fe47b1146100515780636d4ce63c1461007e575b600080fd5b34801561005d57600080fd5b5061007c600480360381019080803590602001909291905050506100a9565b005b34801561008a57600080fd5b50610093610155565b6040518082815260200191505060405180910390f35b7fc6d8c0af6d21f291e7c359603aa97e0ed500f04db6e983b9fce75a91c6b8da6b816040518082815260200191505060405180910390a1806000819055507ffd28ec3ec2555238d8ad6f9faf3e4cd10e574ce7e7ef28b73caa53f9512f65b93382604051808373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020018281526020019250505060405180910390a150565b600080549050905600a165627a7a723058201a1bf0066db9d92d10c43c76eb864787182721a9dcec35b3782d7a8e152c7a850029";
    let code_bytes: Rc<Vec<u8>> = Rc::new(code_str.from_hex().unwrap());
    let address = Address::from_str("ffffffffffffffffffffffffffffffffff020000").unwrap();

    let vm: SeqTransactionVM<MainnetEIP160Patch> = stateful.execute(ValidTransaction {
        caller: None,
        gas_price: Gas::zero(),
        gas_limit: Gas::from(10000000000000u64),
        action: TransactionAction::Create,
        value: U256::zero(),
        input: code_bytes.clone(),
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
        if let AccountChange::Create { nonce, address, balance, storage, code } = account {
            let storage: HashMap<U256, M256> = Into::into(storage.clone());
            println!("address={:?}", address);
            println!("nonce={:?}", nonce);
            println!("balance={:?}", balance);
            println!("storage={:?}", storage);
            println!("code={:?}", code);
        }
    }
}
