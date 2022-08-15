// TODO: checks

use cosmwasm_std::{
    attr, entry_point, to_binary, Addr, Binary, BlockInfo, Coin, CosmosMsg, Deps, DepsMut, Env,
    MessageInfo, Reply, Response, StdError, StdResult, SubMsg, Uint128, WasmMsg,
};
use cw2::set_contract_version;
use cw20::{Cw20ExecuteMsg, Denom, Denom::Cw20};
use cw_storage_plus::Item;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use thiserror::Error;

// Version info for migration
pub const CONTRACT_NAME: &str = "crates.io:mandalorian";
pub const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");


#[derive(Error, Debug, PartialEq)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] StdError),

    #[error("Unbalanced liquidity error")]
    UnbalancedLiquidityError {},
}

//
// State
//

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct Token {
    pub amount: Uint128,
    pub denom: Denom,
}

pub const TOKEN1: Item<Token> = Item::new("token1");
pub const TOKEN2: Item<Token> = Item::new("token2");


//
// Instantiate
//

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct InstantiateMsg {
    pub token1_denom: Denom,
    pub token2_denom: Denom,
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    msg: InstantiateMsg,
) -> Result<Response, ContractError> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    let token1 = Token {
        amount: Uint128::zero(),
        denom: msg.token1_denom.clone(),
    };

    TOKEN1.save(deps.storage, &token1)?;

    let token2 = Token {
        amount: Uint128::zero(),
        denom: msg.token2_denom.clone(),
    };

    TOKEN2.save(deps.storage, &token2)?;

    Ok(Response::new())
}

//
// Execute
//

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub enum TokenSelection {
    Token1,
    Token2,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecuteMsg {
    ProvideLiquidity {
        token1_amount: Uint128,
        token2_amount: Uint128,
    },
    Swap {
        token: TokenSelection,
        input_amount: Uint128,
        min_output: Uint128,
    },
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    msg: ExecuteMsg,
) -> Result<Response, ContractError> {
    match msg {
        ExecuteMsg::ProvideLiquidity {
            token1_amount,
	    token2_amount
        } => provide_liquidity(
            deps,
            &info,
            env,
            token1_amount,
	    token2_amount,
        ),
        ExecuteMsg::Swap {
            token,
            input_amount,
            min_output,
            ..
        } => swap(
            deps,
            &info,
            input_amount,
            env,
            token,
            info.sender.to_string(),
            min_output,
        ),
    }
}

pub fn provide_liquidity(
    deps: DepsMut,
    info: &MessageInfo,
    env: Env,
    token1_amount: Uint128,
    token2_amount: Uint128,
) -> Result<Response, ContractError> {
    let token1 = TOKEN1.load(deps.storage)?;
    let token2 = TOKEN2.load(deps.storage)?;

    if token2_amount * token1.amount != token1_amount * token2.amount {
	return Err(ContractError::UnbalancedLiquidityError {});
    }

    TOKEN1.update(deps.storage, |mut token1| -> Result<_, ContractError> {
        token1.amount += token1_amount;
        Ok(token1)
    })?;
    TOKEN2.update(deps.storage, |mut token2| -> Result<_, ContractError> {
        token2.amount += token2_amount;
        Ok(token2)
    })?;

    let mut transfer_messages: Vec<CosmosMsg> = vec![];
    if let Cw20(addr) = token1.denom {
        transfer_messages.push(make_cw20_transfer_from_message(
            &info.sender,
            &env.contract.address,
            &addr,
            token1_amount,
        )?)
    }
    if let Cw20(addr) = token2.denom {
        transfer_messages.push(make_cw20_transfer_from_message(
            &info.sender,
            &env.contract.address,
            &addr,
            token2_amount,
        )?)
    }

    Ok(Response::new()
       .add_messages(transfer_messages)
       .add_attributes(vec![
           attr("token1_amount", token1_amount),
           attr("token2_amount", token2_amount),
       ]))
}

fn make_cw20_transfer_from_message(
    owner: &Addr,
    recipient: &Addr,
    token_addr: &Addr,
    token_amount: Uint128,
) -> StdResult<CosmosMsg> {
    let transfer_cw20_msg = Cw20ExecuteMsg::TransferFrom {
        owner: owner.into(),
        recipient: recipient.into(),
        amount: token_amount,
    };
    let exec_cw20_transfer = WasmMsg::Execute {
        contract_addr: token_addr.into(),
        msg: to_binary(&transfer_cw20_msg)?,
        funds: vec![],
    };
    let cw20_transfer_cosmos_msg: CosmosMsg = exec_cw20_transfer.into();
    Ok(cw20_transfer_cosmos_msg)
}


#[allow(clippy::too_many_arguments)]
pub fn swap(
    deps: DepsMut,
    info: &MessageInfo,
    input_amount: Uint128,
    _env: Env,
    input_token_selection: TokenSelection,
    recipient: String,
    token1_amount: Uint128,
) -> Result<Response, ContractError> {

    let input_token_item = match input_token_selection {
        TokenSelection::Token1 => TOKEN1,
        TokenSelection::Token2 => TOKEN2,
    };
    let input_token = input_token_item.load(deps.storage)?;
    let output_token_item = match input_token_selection {
        TokenSelection::Token1 => TOKEN2,
        TokenSelection::Token2 => TOKEN1,
    };
    let output_token = output_token_item.load(deps.storage)?;

    let output_token_amount = calculate_output_amount(input_amount, input_token.amount, output_token.amount)?;

    input_token_item.update(
        deps.storage,
        |mut input_token| -> Result<_, ContractError> {
            input_token.amount = input_token
                .amount
                .checked_add(input_amount)
                .map_err(StdError::overflow)?;
            Ok(input_token)
        },
    )?;

    output_token_item.update(
        deps.storage,
        |mut output_token| -> Result<_, ContractError> {
            output_token.amount = output_token
                .amount
                .checked_sub(output_token_amount)
                .map_err(StdError::overflow)?;
            Ok(output_token)
        },
    )?;

    // TODO: send tokens messages

    Ok(Response::new())
}

fn calculate_output_amount(
    input_amount: Uint128,
    input_total: Uint128,
    output_total: Uint128,
) -> StdResult<Uint128> {
    let k = input_total
	.checked_mul(output_total)
	.map_err(StdError::overflow)?;
    let after1 = input_total
	.checked_add(input_amount)
	.map_err(StdError::overflow)?;
    let after2  = k
        .checked_div(after1)
	.map_err(StdError::divide_by_zero)?;
    output_total
        .checked_sub(after2)
	.map_err(StdError::overflow)
}
