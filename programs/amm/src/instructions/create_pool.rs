use crate::error::ErrorCode;
use crate::states::*;
use crate::libraries::tick_math;
use anchor_lang::{prelude::*, Discriminator};
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};
use solana_program::sysvar::instructions;
// use solana_program::{program::invoke_signed, system_instruction};
#[derive(Accounts)]
#[instruction(sqrt_price_x64: u128, open_time: u64, other_ix: u8, leverage: u8)]
pub struct CreatePool<'info> {
    /// Address paying to create the pool. Can be anyone
    #[account(mut)]
    pub pool_creator: Signer<'info>,

    /// Which config the pool belongs to.
    pub amm_config: Box<Account<'info, AmmConfig>>,

    /// Initialize an account to store the pool state
    #[account(
        init,
        seeds = [
            POOL_SEED.as_bytes(),
            amm_config.key().as_ref(),
            token_mint_0.key().as_ref(),
            token_mint_1.key().as_ref(),
            &leverage.to_le_bytes(),
        ],
        bump,
        payer = pool_creator,
        space = PoolState::LEN
    )]
    pub pool_state: AccountLoader<'info, PoolState>,

    /// Token_0 mint, the key must grater then token_1 mint.
    #[account(
        mint::token_program = token_program_0
    )]
    pub token_mint_0: Box<InterfaceAccount<'info, Mint>>,

    /// Token_1 mint
    #[account(
        mint::token_program = token_program_1
    )]
    pub token_mint_1: Box<InterfaceAccount<'info, Mint>>,

    /// Token_0 vault for the pool
    #[account(
        init,
        seeds =[
            POOL_VAULT_SEED.as_bytes(),
            pool_state.key().as_ref(),
            token_mint_0.key().as_ref(),
        ],
        bump,
        payer = pool_creator,
        token::mint = token_mint_0,
        token::authority = pool_state,
        token::token_program = token_program_0,
    )]
    pub token_vault_0: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Token_1 vault for the pool
    #[account(
        init,
        seeds =[
            POOL_VAULT_SEED.as_bytes(),
            pool_state.key().as_ref(),
            token_mint_1.key().as_ref(),
        ],
        bump,
        payer = pool_creator,
        token::mint = token_mint_1,
        token::authority = pool_state,
        token::token_program = token_program_1,
    )]
    pub token_vault_1: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: Initialize an account to store oracle observations, the account must be created off-chain, constract will initialzied it
    #[account(mut)]
    pub observation_state: UncheckedAccount<'info>,

    /// Initialize an account to store if a tick array is initialized.
    #[account(
        init,
        seeds = [
            POOL_TICK_ARRAY_BITMAP_SEED.as_bytes(),
            pool_state.key().as_ref(),
        ],
        bump,
        payer = pool_creator,
        space = TickArrayBitmapExtension::LEN
    )]
    pub tick_array_bitmap: AccountLoader<'info, TickArrayBitmapExtension>,

    /// Spl token program or token program 2022
    pub token_program_0: Interface<'info, TokenInterface>,
    /// Spl token program or token program 2022
    pub token_program_1: Interface<'info, TokenInterface>,
    /// To create a new program account
    pub system_program: Program<'info, System>,
    /// Sysvar for program account
    pub rent: Sysvar<'info, Rent>,
    
    #[account(address = instructions::ID)]
    /// CHECK: check
    pub ixs_sysvar: AccountInfo<'info>,
}

const OTHER_IX_POOL_AI_IDX: usize = 2;

pub fn check_are_we_two_pools(
    pool_key: &Pubkey,
    amm_config_key: &Pubkey,
    token_mint_0_key: &Pubkey,
    token_mint_1_key: &Pubkey,
    sysvar_ixs: &AccountInfo,
    other_ix: usize,
    leverage: u8,
) -> Result<(Pubkey, u8)> {

    let current_ix_idx: usize = instructions::load_current_index_checked(sysvar_ixs)?.into();

    assert!(current_ix_idx != other_ix,         "{}", ErrorCode::NotApproved);

    // Will error if ix doesn't exist
    let unchecked_other_ix_ix = instructions::load_instruction_at_checked(other_ix, sysvar_ixs)?;

    assert!(
        unchecked_other_ix_ix.data[..8]
            .eq(&crate::instruction::CreatePool::DISCRIMINATOR),
            "{}", ErrorCode::NotApproved
        );

    assert!(
        unchecked_other_ix_ix.program_id.eq(&crate::id()),
        "{}", ErrorCode::NotApproved
    );

    let other_ix_ix = unchecked_other_ix_ix;

    let other_ix_pool = other_ix_ix
        .accounts
        .get(OTHER_IX_POOL_AI_IDX)
        .ok_or(ErrorCode::NotApproved)?;

    let (expect_pda_address, _bump) = Pubkey::find_program_address(
            &[
                POOL_SEED.as_bytes(),
                amm_config_key.as_ref(),
                token_mint_1_key.as_ref(),
                token_mint_0_key.as_ref(),
                &leverage.to_le_bytes(),
            ],
            &crate::id(),
        );
        require_keys_eq!(expect_pda_address, other_ix_pool.pubkey);

    assert!(
        other_ix_pool.pubkey.ne(&pool_key),
        "{}", ErrorCode::NotApproved
    );
    let long_or_short =other_ix > current_ix_idx;
    let long_or_short = if long_or_short { 1 } else { 0 };
    Ok((other_ix_pool.pubkey, long_or_short))
}

pub fn create_pool(ctx: Context<CreatePool>, sqrt_price_x64: u128, open_time: u64, other_ix: u8, leverage: u8) -> Result<()> {
   
    let pool_id = ctx.accounts.pool_state.key();
    let mut pool_state = ctx.accounts.pool_state.load_init()?;
    let (other_ix_pubkey_checked, long_or_short) = check_are_we_two_pools(
        &pool_id,
        &ctx.accounts.amm_config.key(),
        &ctx.accounts.token_mint_0.key(),
        &ctx.accounts.token_mint_1.key(),
        &ctx.accounts.ixs_sysvar,
        other_ix as usize,
    leverage)?;
    let tick = tick_math::get_tick_at_sqrt_price(sqrt_price_x64)?;
    #[cfg(feature = "enable-log")]
    msg!(
        "create pool, init_price: {}, init_tick:{}",
        sqrt_price_x64,
        tick
    );
    // init observation
    ObservationState::initialize(ctx.accounts.observation_state.as_ref(), pool_id)?;

    let bump = ctx.bumps.pool_state;
    pool_state.initialize(
        bump,
        sqrt_price_x64,
        open_time,
        tick,
        ctx.accounts.pool_creator.key(),
        ctx.accounts.token_vault_0.key(),
        ctx.accounts.token_vault_1.key(),
        ctx.accounts.amm_config.as_ref(),
        ctx.accounts.token_mint_0.as_ref(),
        ctx.accounts.token_mint_1.as_ref(),
        ctx.accounts.observation_state.key(),
        other_ix_pubkey_checked,
        long_or_short,
        leverage
    )?;

    ctx.accounts
        .tick_array_bitmap
        .load_init()?
        .initialize(pool_id);

    emit!(PoolCreatedEvent {
        token_mint_0: ctx.accounts.token_mint_0.key(),
        token_mint_1: ctx.accounts.token_mint_1.key(),
        tick_spacing: ctx.accounts.amm_config.tick_spacing,
        pool_state: ctx.accounts.pool_state.key(),
        sqrt_price_x64,
        tick,
        token_vault_0: ctx.accounts.token_vault_0.key(),
        token_vault_1: ctx.accounts.token_vault_1.key(),
    });
    Ok(())
}
