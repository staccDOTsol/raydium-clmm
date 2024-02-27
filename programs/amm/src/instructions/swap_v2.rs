use std::collections::VecDeque;
use std::ops::Deref;

use crate::error::ErrorCode;
use crate::libraries::tick_math;
use crate::swap::swap_internal;
use crate::util::*;
use crate::{states::*, util};
use anchor_lang::prelude::*;
use anchor_spl::token::Token;
use anchor_spl::token_interface::{Mint, Token2022, TokenAccount};

/// Memo msg for swap
pub const SWAP_MEMO_MSG: &'static [u8] = b"raydium_swap";
#[derive(Accounts)]
pub struct SwapSingleV2<'info> {
    /// The user performing the swap
    pub payer: Signer<'info>,

    /// The factory state to read protocol fees
    #[account(address = l_state.load()?.amm_config)]
    pub amm_config: Box<Account<'info, AmmConfig>>,

    /// The program account of the pool in which the swap will be performed
    #[account(mut)]
    pub l_state: AccountLoader<'info, PoolState>,

    /// The program account of the pool in which the swap will be performed
    #[account(mut)]
    pub s_state: AccountLoader<'info, PoolState>,

    /// The user token account for input token
    #[account(mut)]
    pub input_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The user token account for output token
    #[account(mut)]
    pub output_token_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The vault token account for input token
    #[account(mut)]
    pub input_vault_l: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The vault token account for output token
    #[account(mut)]
    pub output_vault_l: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The vault token account for input token
    #[account(mut)]
    pub input_vault_s: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The vault token account for output token
    #[account(mut)]
    pub output_vault_s: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The program account for the most recent oracle observation
    #[account(mut, address = l_state.load()?.observation_key)]
    pub observation_state_l: AccountLoader<'info, ObservationState>,

    /// The program account for the most recent oracle observation
    #[account(mut, address = s_state.load()?.observation_key)]
    pub observation_state_s: AccountLoader<'info, ObservationState>,

    /// SPL program for token transfers
    pub token_program: Program<'info, Token>,

    /// SPL program 2022 for token transfers
    pub token_program_2022: Program<'info, Token2022>,

    /// CHECK:
    #[account(
        address = spl_memo::id()
    )]
    pub memo_program: UncheckedAccount<'info>,

    /// The mint of token vault 0

    pub input_vault_mint: Box<InterfaceAccount<'info, Mint>>,

    /// The mint of token vault 1

    pub output_vault_mint: Box<InterfaceAccount<'info, Mint>>,
    // remaining accounts
    // tickarray_bitmap_extension: must add account if need regardless the sequence
    // tick_array_account_1
    // tick_array_account_2
    // tick_array_account_...
}
pub fn rebalanc00r(
    ctx: &mut SwapSingleV2,
    leverage_factor: u64,
    input_vault_l: &TokenAccount,
    output_vault_s: &TokenAccount,
    
) -> Result<()> {
    
    // Ensure the total balance is not zero to avoid division by zero.
    let total_balance = input_vault_l.amount.checked_add(output_vault_s.amount).ok_or(ErrorCode::TotalBalanceZero)?;
    if total_balance == 0 {
        return err!(ErrorCode::TotalBalanceZero);
    }

    // Calculate the target balances for long and short positions.
    let target_long_balance = total_balance.checked_mul(leverage_factor).ok_or(ErrorCode::TotalBalanceZero)?
                            .checked_div(leverage_factor.checked_add(1).ok_or(ErrorCode::TotalBalanceZero)?)
                            .ok_or(ErrorCode::TotalBalanceZero)?;

    let target_short_balance = total_balance.checked_sub(target_long_balance).ok_or(ErrorCode::TotalBalanceZero)?;
    transfer_from_pool_vault_to_user(
        &ctx.l_state,
        &ctx.input_vault_l,
        &ctx.output_vault_s,
        None,
        &ctx.token_program,
        Some(ctx.token_program_2022.to_account_info()),
        // transfer target_long_balance from long_vault to short_vault
        target_long_balance,
    )?;
    transfer_from_pool_vault_to_user(
        &ctx.s_state,
        &ctx.output_vault_s,
        &ctx.input_vault_l,
        None,
        &ctx.token_program,
        Some(ctx.token_program_2022.to_account_info()),
        // transfer target_short_balance from short_vault to long_vault
        target_short_balance,
    )?;
    Ok(())
 }pub fn adjust_position_values(
    long_pool: &mut PoolState,
    short_pool: &mut PoolState,
) -> Result<()> {
    let new_price = long_pool.sqrt_price_x64;

    // Calculate the price movement ratio as a fixed-point number
    // to avoid floating-point operations. This requires defining a precision factor.
    let precision_factor = 1_000_000; // 6 decimal places for example

    let price_movement_ratio = new_price.checked_mul(precision_factor)
                                        .ok_or(ErrorCode::TooMuchInputPaid)?
                                        .checked_div(long_pool.last_price)
                                        .ok_or(ErrorCode::TooMuchInputPaid)?;

    // Adjust the long pool's value
    long_pool.value = long_pool.value.checked_mul(price_movement_ratio)
                                    .ok_or(ErrorCode::TooMuchInputPaid)?
                                    .checked_div(precision_factor) // Adjust back by the precision factor
                                    .ok_or(ErrorCode::TooMuchInputPaid)?;

    // Adjust the short pool's value inversely
    short_pool.value = short_pool.value.checked_mul(precision_factor) // Adjust by the precision factor
                                        .ok_or(ErrorCode::TooMuchInputPaid)?
                                        .checked_div(price_movement_ratio)
                                        .ok_or(ErrorCode::TooMuchInputPaid)?;

    // Update last prices
    long_pool.last_price = new_price;
    short_pool.last_price = new_price;

    Ok(())
}


/// Performs a single exact input/output swap
/// if is_base_input = true, return vaule is the max_amount_out, otherwise is min_amount_in
pub fn exact_internal_v2<'c: 'info, 'info>(
    ctx: &mut SwapSingleV2<'info>,
    remaining_accounts: &'c [AccountInfo<'info>],
    amount_specified: u64,
    sqrt_price_limit_x64: u128,
    is_base_input: bool,
) -> Result<u64> {
    // invoke_memo_instruction(SWAP_MEMO_MSG, ctx.memo_program.to_account_info())?;

    let block_timestamp = solana_program::clock::Clock::get()?.unix_timestamp as u64;
    let input_vault_mint = ctx.input_vault_mint.clone();
    let output_vault_mint = ctx.output_vault_mint.clone();
    let amount_0;
    let amount_1;
    let zero_for_one;
    let swap_price_before;

    let input_balance_before = ctx.input_token_account.amount;
    let output_balance_before = ctx.output_token_account.amount;

    // calculate specified amount because the amount includes thransfer_fee as input and without thransfer_fee as output
    let amount_specified = if is_base_input {
        let transfer_fee =
            util::get_transfer_fee(input_vault_mint.clone(), amount_specified).unwrap();
        amount_specified - transfer_fee
    } else {
        let transfer_fee =
            util::get_transfer_inverse_fee(output_vault_mint.clone(), amount_specified)
                .unwrap();
        amount_specified + transfer_fee
    };
    let use_long_vaults = (Clock::get()?.unix_timestamp % 2) == 0;
    let input_vault;
    let output_vault;
    let pool_state;
    let observation_state;
    let leverage_factor = 100;
    let other_pool_state;
    if use_long_vaults {
        input_vault = ctx.input_vault_l.clone();
        output_vault = ctx.output_vault_l.clone();
        pool_state = ctx.l_state.clone();
        observation_state = ctx.observation_state_l.clone();
        other_pool_state = ctx.s_state.clone();
        rebalanc00r(
            ctx,
            leverage_factor,
            &input_vault,
            &ctx.output_vault_s.clone(),
        )?;
        rebalanc00r(
            ctx,
            leverage_factor,
            &output_vault,
            &ctx.input_vault_s.clone(),
        )?;
    } else {
        input_vault = ctx.input_vault_s.clone();
        output_vault = ctx.output_vault_s.clone();
        pool_state = ctx.s_state.clone();
        observation_state = ctx.observation_state_s.clone();
        other_pool_state = ctx.l_state.clone();
        rebalanc00r(
            ctx,
            leverage_factor,
            &ctx.output_vault_l.clone(),
            &input_vault,
        )?;
        rebalanc00r(
            ctx,
            leverage_factor,
            &ctx.input_vault_l.clone(),
            &output_vault,
        )?;
    }
    {
        swap_price_before = pool_state.load()?.sqrt_price_x64;
        let pool_state = &mut pool_state.load_mut()?;
        adjust_position_values(pool_state, &mut other_pool_state.load_mut().unwrap())?;
        
        zero_for_one = input_vault.mint == pool_state.token_mint_0;

        require_gt!(block_timestamp, pool_state.open_time);

        require!(
            if zero_for_one {
                input_vault.key() == pool_state.token_vault_0
                    && output_vault.key() == pool_state.token_vault_1
            } else {
                input_vault.key() == pool_state.token_vault_1
                    && output_vault.key() == pool_state.token_vault_0
            },
            ErrorCode::InvalidInputPoolVault
        );

        let mut tickarray_bitmap_extension = None;
        let tick_array_states = &mut VecDeque::new();

        let tick_array_bitmap_extension_key = TickArrayBitmapExtension::key(pool_state.key());
        for account_info in remaining_accounts.into_iter() {
            if account_info.key().eq(&tick_array_bitmap_extension_key) {
                tickarray_bitmap_extension = Some(
                    *(AccountLoader::<TickArrayBitmapExtension>::try_from(account_info)?
                        .load()?
                        .deref()),
                );
                continue;
            }
            tick_array_states.push_back(AccountLoad::load_data_mut(account_info)?);
        }

        (amount_0, amount_1) = swap_internal(
            &ctx.amm_config,
            pool_state,
            tick_array_states,
            &mut observation_state.load_mut()?,
            &tickarray_bitmap_extension,
            amount_specified,
            if sqrt_price_limit_x64 == 0 {
                if zero_for_one {
                    tick_math::MIN_SQRT_PRICE_X64 + 1
                } else {
                    tick_math::MAX_SQRT_PRICE_X64 - 1
                }
            } else {
                sqrt_price_limit_x64
            },
            zero_for_one,
            is_base_input,
            oracle::block_timestamp(),
        )?;

        #[cfg(feature = "enable-log")]
        msg!(
            "exact_swap_internal, is_base_input:{}, amount_0: {}, amount_1: {}",
            is_base_input,
            amount_0,
            amount_1
        );
        require!(
            amount_0 != 0 && amount_1 != 0,
            ErrorCode::TooSmallInputOrOutputAmount
        );
    }
    let (token_account_0, token_account_1, vault_0, vault_1, vault_0_mint, vault_1_mint) =
        if zero_for_one {
            (
                ctx.input_token_account.clone(),
                ctx.output_token_account.clone(),
                input_vault.clone(),
                output_vault.clone(),
                input_vault_mint.clone(),
                output_vault_mint.clone(),
            )
        } else {
            (
                ctx.output_token_account.clone(),
                ctx.input_token_account.clone(),
                output_vault.clone(),
                input_vault.clone(),
                output_vault_mint.clone(),
                input_vault_mint.clone(),
            )
        };

    // user or pool real amount delta without tranfer fee
    let amount_0_without_fee;
    let amount_1_without_fee;
    // the transfer fee amount charged by withheld_amount
    let transfer_fee_0;
    let transfer_fee_1;
    if zero_for_one {
        transfer_fee_0 = util::get_transfer_inverse_fee(vault_0_mint.clone(), amount_0).unwrap();
        transfer_fee_1 = util::get_transfer_fee(vault_1_mint.clone(), amount_1).unwrap();

        amount_0_without_fee = amount_0;
        amount_1_without_fee = amount_1.checked_sub(transfer_fee_1).unwrap();
        let (transfer_amount_0, transfer_amount_1) = (amount_0 + transfer_fee_0, amount_1);
        #[cfg(feature = "enable-log")]
        msg!(
            "amount_0:{}, transfer_fee_0:{}, amount_1:{}, transfer_fee_1:{}",
            amount_0,
            transfer_fee_0,
            amount_1,
            transfer_fee_1
        );
        //  x -> y, deposit x token from user to pool vault.
        transfer_from_user_to_pool_vault(
            &ctx.payer,
            &token_account_0,
            &vault_0,
            Some(vault_0_mint),
            &ctx.token_program,
            Some(ctx.token_program_2022.to_account_info()),
            transfer_amount_0,
        )?;
        if vault_1.amount <= transfer_amount_1 {
            // freeze pool, disable all instructions
            pool_state.load_mut()?.set_status(255);
        }
        // x -> y，transfer y token from pool vault to user.
        transfer_from_pool_vault_to_user(
            &pool_state,
            &vault_1,
            &token_account_1,
            Some(vault_1_mint),
            &ctx.token_program,
            Some(ctx.token_program_2022.to_account_info()),
            transfer_amount_1,
        )?;
    } else {
        transfer_fee_0 = util::get_transfer_fee(vault_0_mint.clone(), amount_0).unwrap();
        transfer_fee_1 = util::get_transfer_inverse_fee(vault_1_mint.clone(), amount_1).unwrap();

        amount_0_without_fee = amount_0.checked_sub(transfer_fee_0).unwrap();
        amount_1_without_fee = amount_1;
        let (transfer_amount_0, transfer_amount_1) = (amount_0, amount_1 + transfer_fee_1);

        msg!("amount_0:{}, transfer_fee_0:{}", amount_0, transfer_fee_0);
        msg!("amount_1:{}, transfer_fee_1:{}", amount_1, transfer_fee_1);
        transfer_from_user_to_pool_vault(
            &ctx.payer,
            &token_account_1,
            &vault_1,
            Some(vault_1_mint),
            &ctx.token_program,
            Some(ctx.token_program_2022.to_account_info()),
            transfer_amount_1,
        )?;
        if vault_0.amount <= transfer_amount_0 {
            // freeze pool, disable all instructions
            pool_state.load_mut()?.set_status(255);
        }
        transfer_from_pool_vault_to_user(
            &pool_state,
            &vault_0,
            &token_account_0,
            Some(vault_0_mint),
            &ctx.token_program,
            Some(ctx.token_program_2022.to_account_info()),
            transfer_amount_0,
        )?;
    }
    ctx.output_token_account.reload()?;
    ctx.input_token_account.reload()?;

    let pool_state = pool_state.load()?;


    emit!(SwapEvent {
        pool_state: pool_state.key(),
        sender: ctx.payer.key(),
        token_account_0: token_account_0.key(),
        token_account_1: token_account_1.key(),
        amount_0: amount_0_without_fee,
        transfer_fee_0,
        amount_1: amount_1_without_fee,
        transfer_fee_1,
        zero_for_one,
        sqrt_price_x64: pool_state.sqrt_price_x64,
        liquidity: pool_state.liquidity,
        tick: pool_state.tick_current
    });
    if zero_for_one {
        require_gt!(swap_price_before, pool_state.sqrt_price_x64);
    } else {
        require_gt!(pool_state.sqrt_price_x64, swap_price_before);
    }

    if is_base_input {
        Ok(ctx
            .output_token_account
            .amount
            .checked_sub(output_balance_before)
            .unwrap())
    } else {
        Ok(input_balance_before
            .checked_sub(ctx.input_token_account.amount)
            .unwrap())
    }
}

pub fn swap_v2<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, SwapSingleV2<'info>>,
    amount: u64,
    other_amount_threshold: u64,
    sqrt_price_limit_x64: u128,
    is_base_input: bool,
) -> Result<()> {
    let amount_result = exact_internal_v2(
        ctx.accounts,
        ctx.remaining_accounts,
        amount,
        sqrt_price_limit_x64,
        is_base_input,
    )?;
    if is_base_input {
        require_gte!(
            amount_result,
            other_amount_threshold,
            ErrorCode::TooLittleOutputReceived
        );
    } else {
        require_gte!(
            other_amount_threshold,
            amount_result,
            ErrorCode::TooMuchInputPaid
        );
    }

    Ok(())
}
