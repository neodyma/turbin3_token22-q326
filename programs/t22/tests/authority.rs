//! Two extensions about who is allowed to move your tokens.
//!
//! CpiGuard is a defence the account owner switches on.
//! PermanentDelegate is a power the mint creator holds over every holder.
//! They point in opposite directions, which is why I put them together here.

use anchor_lang::{
    prelude::Pubkey,
    solana_program::{instruction::Instruction, system_program},
    InstructionData, ToAccountMetas,
};
use litesvm::LiteSVM;
use solana_keypair::Keypair;
use solana_message::Message;
use solana_signer::Signer;
use solana_transaction::Transaction;
use t22::{accounts, instruction, ID};
use t22new::{
    extension::{
        cpi_guard::instruction::{disable_cpi_guard, enable_cpi_guard},
        BaseStateWithExtensions, ExtensionType, StateWithExtensions,
    },
    instruction::{approve, initialize_account3, mint_to},
    state::Account as TokenAccountState,
};

const TOKEN_2022_PROGRAM_ID: Pubkey = anchor_spl::token_interface::spl_token_2022::ID;
const DECIMALS: u8 = 6;

fn setup() -> (LiteSVM, Keypair) {
    let mut svm = LiteSVM::new();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/deploy/t22.so");
    svm.add_program_from_file(ID, path).unwrap();
    (svm, payer)
}

fn send(svm: &mut LiteSVM, payer: &Keypair, ixs: &[Instruction], extra: &[&Keypair]) {
    let mut signers: Vec<&Keypair> = vec![payer];
    signers.extend_from_slice(extra);
    let bh = svm.latest_blockhash();
    let mut tx = Transaction::new_unsigned(Message::new(ixs, Some(&payer.pubkey())));
    tx.try_sign(&signers, bh).unwrap();
    if let Err(e) = svm.send_transaction(tx) {
        panic!("tx failed: {:#?}", e.meta.logs);
    }
}

fn send_expecting_failure(
    svm: &mut LiteSVM,
    payer: &Keypair,
    ixs: &[Instruction],
    extra: &[&Keypair],
) -> String {
    let mut signers: Vec<&Keypair> = vec![payer];
    signers.extend_from_slice(extra);
    let bh = svm.latest_blockhash();
    let mut tx = Transaction::new_unsigned(Message::new(ixs, Some(&payer.pubkey())));
    tx.try_sign(&signers, bh).unwrap();
    match svm.send_transaction(tx) {
        Ok(_) => panic!("expected failure, got success"),
        Err(e) => e.meta.logs.join("\n"),
    }
}

/// Create a token account carrying CpiGuard, and mint tokens into it.
fn funded_account_with_cpi_guard(
    svm: &mut LiteSVM,
    payer: &Keypair,
    mint: &Pubkey,
    owner: &Keypair,
    amount: u64,
) -> Pubkey {
    let ta = Keypair::new();
    let space =
        ExtensionType::try_calculate_account_len::<TokenAccountState>(&[ExtensionType::CpiGuard])
            .unwrap();
    let lamports = svm.minimum_balance_for_rent_exemption(space);
    send(
        svm,
        payer,
        &[
            solana_system_interface::instruction::create_account(
                &payer.pubkey(),
                &ta.pubkey(),
                lamports,
                space as u64,
                &TOKEN_2022_PROGRAM_ID,
            ),
            initialize_account3(&TOKEN_2022_PROGRAM_ID, &ta.pubkey(), mint, &owner.pubkey())
                .unwrap(),
            mint_to(
                &TOKEN_2022_PROGRAM_ID,
                mint,
                &ta.pubkey(),
                &payer.pubkey(),
                &[],
                amount,
            )
            .unwrap(),
        ],
        &[&ta],
    );
    ta.pubkey()
}

fn plain_account(svm: &mut LiteSVM, payer: &Keypair, mint: &Pubkey, owner: &Pubkey) -> Pubkey {
    let ta = Keypair::new();
    let space = ExtensionType::try_calculate_account_len::<TokenAccountState>(&[]).unwrap();
    let lamports = svm.minimum_balance_for_rent_exemption(space);
    send(
        svm,
        payer,
        &[
            solana_system_interface::instruction::create_account(
                &payer.pubkey(),
                &ta.pubkey(),
                lamports,
                space as u64,
                &TOKEN_2022_PROGRAM_ID,
            ),
            initialize_account3(&TOKEN_2022_PROGRAM_ID, &ta.pubkey(), mint, owner).unwrap(),
        ],
        &[&ta],
    );
    ta.pubkey()
}

fn read_account(svm: &LiteSVM, addr: &Pubkey) -> (u64, Option<Pubkey>, Vec<ExtensionType>) {
    let acct = svm.get_account(addr).unwrap();
    let state = StateWithExtensions::<TokenAccountState>::unpack(&acct.data).unwrap();
    (
        state.base.amount,
        state.base.delegate.into(),
        state.get_extension_types().unwrap(),
    )
}

fn create_plain_mint(svm: &mut LiteSVM, payer: &Keypair) -> Keypair {
    let mint = Keypair::new();
    send(
        svm,
        payer,
        &[Instruction {
            program_id: ID,
            accounts: accounts::CreateMintDeclarative {
                payer: payer.pubkey(),
                mint: mint.pubkey(),
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: instruction::CreateMintDeclarative { decimals: DECIMALS }.data(),
        }],
        &[&mint],
    );
    mint
}

fn create_seizable_mint(svm: &mut LiteSVM, payer: &Keypair) -> Keypair {
    let mint = Keypair::new();
    send(
        svm,
        payer,
        &[Instruction {
            program_id: ID,
            accounts: accounts::CreateSeizableMint {
                payer: payer.pubkey(),
                mint: mint.pubkey(),
                token_program: TOKEN_2022_PROGRAM_ID,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: instruction::CreateSeizableMint { decimals: DECIMALS }.data(),
        }],
        &[&mint],
    );
    mint
}

fn delegate_via_cpi_ix(token_account: &Pubkey, delegate: &Pubkey, owner: &Pubkey) -> Instruction {
    Instruction {
        program_id: ID,
        accounts: accounts::DelegateToProgram {
            token_account: *token_account,
            delegate: *delegate,
            owner: *owner,
            token_program: TOKEN_2022_PROGRAM_ID,
        }
        .to_account_metas(None),
        data: instruction::DelegateToProgram { amount: 500 }.data(),
    }
}

#[test]
fn without_cpi_guard_a_program_can_take_delegation() {
    let (mut svm, payer) = setup();
    let mint = create_plain_mint(&mut svm, &payer);
    let ta = funded_account_with_cpi_guard(&mut svm, &payer, &mint.pubkey(), &payer, 1_000);
    let delegate = Pubkey::new_unique();

    // Account extensions are the mirror image of mint extensions. A mint
    // extension must be initialized BEFORE InitializeMint, and the space and
    // the TLV entry appear together. CpiGuard only needs the space reserved up
    // front; the TLV entry does not exist until EnableCpiGuard is issued after
    // the account is initialized.
    let (_, _, extensions) = read_account(&svm, &ta);
    assert!(
        extensions.is_empty(),
        "space reserved, but no TLV entry yet"
    );

    send(
        &mut svm,
        &payer,
        &[delegate_via_cpi_ix(&ta, &delegate, &payer.pubkey())],
        &[],
    );

    let (_, stored_delegate, _) = read_account(&svm, &ta);
    assert_eq!(stored_delegate, Some(delegate));
}

#[test]
fn cpi_guard_blocks_delegation_issued_through_a_cpi() {
    let (mut svm, payer) = setup();
    let mint = create_plain_mint(&mut svm, &payer);
    let ta = funded_account_with_cpi_guard(&mut svm, &payer, &mint.pubkey(), &payer, 1_000);
    let delegate = Pubkey::new_unique();

    send(
        &mut svm,
        &payer,
        &[enable_cpi_guard(&TOKEN_2022_PROGRAM_ID, &ta, &payer.pubkey(), &[]).unwrap()],
        &[],
    );

    // Now the extension exists.
    let (_, _, extensions) = read_account(&svm, &ta);
    assert_eq!(extensions, vec![ExtensionType::CpiGuard]);

    let logs = send_expecting_failure(
        &mut svm,
        &payer,
        &[delegate_via_cpi_ix(&ta, &delegate, &payer.pubkey())],
        &[],
    );
    assert!(
        logs.contains("CpiGuard") || logs.contains("Approve"),
        "blocked for the wrong reason:\n{logs}"
    );

    // No delegate was recorded.
    let (_, stored_delegate, _) = read_account(&svm, &ta);
    assert_eq!(stored_delegate, None);
}

#[test]
fn cpi_guard_still_allows_the_owner_to_approve_directly() {
    let (mut svm, payer) = setup();
    let mint = create_plain_mint(&mut svm, &payer);
    let ta = funded_account_with_cpi_guard(&mut svm, &payer, &mint.pubkey(), &payer, 1_000);
    let delegate = Pubkey::new_unique();

    send(
        &mut svm,
        &payer,
        &[enable_cpi_guard(&TOKEN_2022_PROGRAM_ID, &ta, &payer.pubkey(), &[]).unwrap()],
        &[],
    );

    // Same Approve, but issued as a top level instruction the owner signed
    // rather than through a program. CpiGuard permits it.
    send(
        &mut svm,
        &payer,
        &[approve(
            &TOKEN_2022_PROGRAM_ID,
            &ta,
            &delegate,
            &payer.pubkey(),
            &[],
            500,
        )
        .unwrap()],
        &[],
    );

    let (_, stored_delegate, _) = read_account(&svm, &ta);
    assert_eq!(stored_delegate, Some(delegate));
}

#[test]
fn cpi_guard_can_be_switched_off_again_by_the_owner() {
    let (mut svm, payer) = setup();
    let mint = create_plain_mint(&mut svm, &payer);
    let ta = funded_account_with_cpi_guard(&mut svm, &payer, &mint.pubkey(), &payer, 1_000);
    let delegate = Pubkey::new_unique();

    send(
        &mut svm,
        &payer,
        &[enable_cpi_guard(&TOKEN_2022_PROGRAM_ID, &ta, &payer.pubkey(), &[]).unwrap()],
        &[],
    );
    send(
        &mut svm,
        &payer,
        &[disable_cpi_guard(&TOKEN_2022_PROGRAM_ID, &ta, &payer.pubkey(), &[]).unwrap()],
        &[],
    );
    send(
        &mut svm,
        &payer,
        &[delegate_via_cpi_ix(&ta, &delegate, &payer.pubkey())],
        &[],
    );

    let (_, stored_delegate, _) = read_account(&svm, &ta);
    assert_eq!(stored_delegate, Some(delegate));
}

#[test]
fn a_permanent_delegate_moves_tokens_without_consent() {
    let (mut svm, payer) = setup();

    // The mint's permanent delegate is the payer, set declaratively.
    let mint = create_seizable_mint(&mut svm, &payer);

    // A holder who has nothing to do with the mint authority.
    let victim = Keypair::new();
    svm.airdrop(&victim.pubkey(), 1_000_000_000).unwrap();
    let victim_account = plain_account(&mut svm, &payer, &mint.pubkey(), &victim.pubkey());
    let attacker_account = plain_account(&mut svm, &payer, &mint.pubkey(), &payer.pubkey());
    send(
        &mut svm,
        &payer,
        &[mint_to(
            &TOKEN_2022_PROGRAM_ID,
            &mint.pubkey(),
            &victim_account,
            &payer.pubkey(),
            &[],
            1_000,
        )
        .unwrap()],
        &[],
    );

    let (before, delegate, _) = read_account(&svm, &victim_account);
    assert_eq!(before, 1_000);
    // No Approve was ever issued.
    assert_eq!(delegate, None);

    // The victim does not sign this transaction.
    send(
        &mut svm,
        &payer,
        &[Instruction {
            program_id: ID,
            accounts: accounts::PermanentDelegateSeize {
                source: victim_account,
                mint: mint.pubkey(),
                destination: attacker_account,
                permanent_delegate: payer.pubkey(),
                token_program: TOKEN_2022_PROGRAM_ID,
            }
            .to_account_metas(None),
            data: instruction::PermanentDelegateSeize {
                amount: 1_000,
                decimals: DECIMALS,
            }
            .data(),
        }],
        &[],
    );

    let (after, _, _) = read_account(&svm, &victim_account);
    let (taken, _, _) = read_account(&svm, &attacker_account);
    assert_eq!(after, 0);
    assert_eq!(taken, 1_000);
    println!("permanent delegate moved 1000 with no approval and no holder signature");
}

#[test]
fn wrong_delegate_cannot_seize() {
    let (mut svm, payer) = setup();
    let mint = create_seizable_mint(&mut svm, &payer);
    let holder = Keypair::new();
    let wrong_delegate = Keypair::new();
    let source = plain_account(&mut svm, &payer, &mint.pubkey(), &holder.pubkey());
    let destination = plain_account(&mut svm, &payer, &mint.pubkey(), &payer.pubkey());
    send(
        &mut svm,
        &payer,
        &[mint_to(
            &TOKEN_2022_PROGRAM_ID,
            &mint.pubkey(),
            &source,
            &payer.pubkey(),
            &[],
            1_000,
        )
        .unwrap()],
        &[],
    );

    let before_source = svm.get_account(&source).unwrap().data;
    let before_destination = svm.get_account(&destination).unwrap().data;
    let logs = send_expecting_failure(
        &mut svm,
        &payer,
        &[Instruction {
            program_id: ID,
            accounts: accounts::PermanentDelegateSeize {
                source,
                mint: mint.pubkey(),
                destination,
                permanent_delegate: wrong_delegate.pubkey(),
                token_program: TOKEN_2022_PROGRAM_ID,
            }
            .to_account_metas(None),
            data: instruction::PermanentDelegateSeize {
                amount: 500,
                decimals: DECIMALS,
            }
            .data(),
        }],
        &[&wrong_delegate],
    );
    assert!(logs.contains("owner does not match") || logs.contains("OwnerMismatch"), "unexpected failure:\n{logs}");
    assert_eq!(svm.get_account(&source).unwrap().data, before_source);
    assert_eq!(svm.get_account(&destination).unwrap().data, before_destination);
}
