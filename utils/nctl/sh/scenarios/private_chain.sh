#!/usr/bin/env bash

source "$NCTL"/sh/scenarios/common/itst.sh

# Exit if any of the commands fail.
set -e

#######################################
# Runs an integration test that exercises
# the private chain functionality.
#
#######################################

function main() {
    log "------------------------------------------------------------"
    log "Starting Scenario: private_chain"
    log "------------------------------------------------------------"

    # 0. Wait for network start up
    do_await_genesis_era_to_complete

    # 1. Check admin can deploy wasm and is fully refunded.
    #    ... Full refund expected due to fee and refund handling
    #    ... going to back to administrator account
    check_admin_wasm_deploy

    # 2. Check admin can send a native deploy and that the
    #    ... expected balance matches after fee_handling
    #    ... is distributed.
    check_admin_native_transfer

    # 3. Check that auction bids are restricted.
    #    ... Expected to have the deploy fail.
    check_bid_failed '6' '1000' '0'

    # 4. Check that transfers are restricted.
    #    ... Expected to have the deploy fail.
    check_transfer_failed '1' '2' '2500000000'

    # 5. Check that rewards are turned off.
    #    ... Validator weights are expected to remain
    #    ... the same.
    check_no_rewards

    log "------------------------------------------------------------"
    log "Scenario private_chain complete"
    log "------------------------------------------------------------"
}

# Sends a bid for a node and returns the deploy hash
function send_bid() {
    local NODE_ID=${1}
    local BID_AMOUNT=${2}
    local BID_DELEGATION_RATE=${3}
    local OUTPUT

    OUTPUT=$(source "$NCTL"/sh/contracts-auction/do_bid.sh \
        node="$NODE_ID" \
        amount="$BID_AMOUNT" \
        rate="$BID_DELEGATION_RATE" \
        quiet="FALSE")

    echo "$OUTPUT" | grep 'deploy hash =' | awk -F'=' '{print $2}' | tr -d ' '
}

# Sends a sigle native transfer and returns the deploy hash
function send_single_native_transfer() {
    local SEND_FROM=${1}
    local SEND_TO=${2}
    local AMOUNT=${3}
    local SECRET_KEY_PATH
    local OUTPUT

    # The faucet is being used as the admin key
    if [ "$SEND_FROM" = 'admin' ]; then
        SECRET_KEY_PATH="$(get_path_to_faucet)/secret_key.pem"
    else
        SECRET_KEY_PATH="$(get_path_to_user $SEND_FROM)/secret_key.pem"
    fi

    OUTPUT=$($(get_path_to_client) transfer \
        --node-address "$(get_node_address_rpc)" \
        --amount "$AMOUNT" \
        --chain-name "$(get_chain_name)" \
        --payment-amount '10000' \
        --target-account "$(cat $(get_path_to_user $SEND_TO)/public_key_hex)" \
        --secret-key "$SECRET_KEY_PATH" \
        --transfer-id '1')

    echo "$OUTPUT" | jq -r '.result.deploy_hash'
}

# Checks that all validator weights remained the same during the test.
# This is expected with rewards being turned off.
function check_no_rewards() {
    local UNIQ_VALIDATOR_WEIGHTS
    local EXPECTED_WEIGHT

    log_step "Checking rewards are disabled"

    EXPECTED_WEIGHT='2'

    UNIQ_VALIDATOR_WEIGHTS=$(nctl-view-chain-auction-info \
        | jq -r '.auction_state.era_validators[].validator_weights[].weight' \
        | sort \
        | uniq )

    if [ "$EXPECTED_WEIGHT" = "$UNIQ_VALIDATOR_WEIGHTS" ]; then
        log "... validator weight remained the same: $UNIQ_VALIDATOR_WEIGHTS [expected]"
    else
        log "ERROR: Validator weights changed! Rewards are happening!"
        exit 1
    fi
}

# Gets balance for specified key and returns the balance
function query_current_balance() {
    local PUBLIC_KEY_HEX=${1}
    local OUTPUT

    OUTPUT=$($(get_path_to_client) query-balance \
        --node-address "$(get_node_address_rpc)" \
        --purse-identifier "$PUBLIC_KEY_HEX")

    echo $OUTPUT | jq -r '.result.balance'
}

# Checks that the admin account can deploy wasm successfully.
# Also verifys the payment and fees are give to the admin account.
function check_admin_wasm_deploy() {
    local CONTRACT_PATH
    local SIGNER_SECRET_KEY_PATH
    local OUTPUT
    local DEPLOY_HASH
    local EXPECTED_BALANCE
    local ACTUAL_BALANCE

    log_step "Checking admin wasm deploy"

    EXPECTED_BALANCE='1000000000000000000000000000000000'
    CONTRACT_PATH="$NCTL_CASPER_HOME/target/wasm32-unknown-unknown/release/nctl-dictionary.wasm"
    SIGNER_SECRET_KEY_PATH="$(get_path_to_faucet)/secret_key.pem"

    OUTPUT=$($(get_path_to_client) put-deploy \
        --node-address "$(get_node_address_rpc)" \
        --chain-name "$(get_chain_name)" \
        --payment-amount '100000000000' \
        --session-path "$CONTRACT_PATH" \
        --secret-key "$SIGNER_SECRET_KEY_PATH")

    DEPLOY_HASH=$(echo "$OUTPUT" | jq -r '.result.deploy_hash')
    await_deploy_inclusion "$DEPLOY_HASH"
    nctl-await-n-eras offset='1' sleep_interval='5.0' timeout='180'

    ACTUAL_BALANCE=$(query_current_balance $(cat $(get_path_to_faucet)/public_key_hex))

    if [ "$EXPECTED_BALANCE" = "$ACTUAL_BALANCE"  ]; then
        log "... verified admin account is back to starting balance: $ACTUAL_BALANCE [expected]"
    else
        log "ERROR: admin account balance differs from expected!"
        exit 1
    fi
}

# Checks admin can send a native trasnfer and that
# the fees are give back to him.
function check_admin_native_transfer() {
    local DEPLOY_HASH
    local ADMIN_KEY_HEX
    local SEND_TO_KEY_HEX
    local ADMIN_POST_BALANCE
    local SEND_TO_POST_BALANCE
    local EXPECTED_SEND_TO_BALANCE
    local EXPECTED_ADMIN_BALANCE
    local AMOUNT

    log_step "Checking admin can transfer"

    AMOUNT='2500000000'
    ADMIN_KEY_HEX=$(cat $(get_path_to_faucet)/public_key_hex)
    SEND_TO_KEY_HEX=$(cat $(get_path_to_user '2')/public_key_hex)
    DEPLOY_HASH=$(send_single_native_transfer 'admin' '2' "$AMOUNT")
    EXPECTED_SEND_TO_BALANCE='1000000000000000000000002500000000'
    EXPECTED_ADMIN_BALANCE='999999999999999999999997500000000'

    await_deploy_inclusion "$DEPLOY_HASH"
    nctl-await-n-eras offset='1' sleep_interval='5.0' timeout='180'

    ADMIN_POST_BALANCE=$(query_current_balance "$ADMIN_KEY_HEX")
    SEND_TO_POST_BALANCE=$(query_current_balance "$SEND_TO_KEY_HEX")

    # check target balance
    if [ "$EXPECTED_SEND_TO_BALANCE" = "$SEND_TO_POST_BALANCE"  ]; then
        log "... verified target account increased by $AMOUNT"
    else
        log "ERROR: target account did not increase as expected!"
        exit 1
    fi

    # check admin balance - should be exact due to 'accumulate' chainspec setting
    # ... this setting gives the fee collected back to the admin accounts
    # ... currently set to wasmless_transfer_cost = 100_000_000
    if [ "$EXPECTED_ADMIN_BALANCE" = "$ADMIN_POST_BALANCE"  ]; then
        log "... verified admin account decreased by $AMOUNT"
    else
        log "ERROR: admin account did not decrease as expected!"
        exit 1
    fi
}

# Checks that a non-admin can not send a transfer.
# Will fail with a specific message which we check for.
function check_transfer_failed() {
    local SEND_FROM=${1}
    local SEND_TO=${2}
    local AMOUNT=${3}
    local DEPLOY_HASH
    local EXPECTED_ERROR
    local ACTUAL_ERROR

    EXPECTED_ERROR='Failed to transfer with unrestricted transfers disabled'

    log_step "Attempting to send transfer, the deploy should fail"

    DEPLOY_HASH=$(send_single_native_transfer "$SEND_FROM" "$SEND_TO" "$AMOUNT")

    await_deploy_inclusion "$DEPLOY_HASH"
    ACTUAL_ERROR=$($(get_path_to_client) get-deploy \
        --node-address "$(get_node_address_rpc)" \
        "$DEPLOY_HASH" \
        | jq -r '.result.execution_results[].result.Failure.error_message')

    if grep -q "$EXPECTED_ERROR" <<< "$ACTUAL_ERROR"; then
        log "... deploy errored with: $ACTUAL_ERROR [expected]"
    else
        log "ERROR: Deploy didn't error as expected!"
        exit 1
    fi
}

# Checks that bids are disable.
# Will fail with a specific message which we check for.
# We also double check that the key is not in auction state
function check_bid_failed() {
    local NODE_ID=${1}
    local BID_AMOUNT=${2}
    local BID_DELEGATION_RATE=${3}
    local EXPECTED_ERROR
    local ACTUAL_ERROR
    local DEPLOY_HASH

    # part of the message to check for
    EXPECTED_ERROR='AuctionBidsDisabled'

    log_step 'Checking bid failed'

    DEPLOY_HASH=$(send_bid "$NODE_ID" "$BID_AMOUNT" "$BID_DELEGATION_RATE")

    await_deploy_inclusion "$DEPLOY_HASH"

    ACTUAL_ERROR=$($(get_path_to_client) get-deploy \
        --node-address "$(get_node_address_rpc)" \
        "$DEPLOY_HASH" \
        | jq -r '.result.execution_results[].result.Failure.error_message')

    if grep -q "$EXPECTED_ERROR" <<< "$ACTUAL_ERROR"; then
        log "... deploy errored with: $ACTUAL_ERROR [expected]"
    else
        log "ERROR: Bid deploy didn't error as expected!"
        exit 1
    fi

    log "... waiting to see if bid shows up in auction"
    # validator would bid in by era 6 auction info normally, 
    # so era 3 will have it by then
    nctl-await-until-era-n era='3' log='true'

    if $(assert_new_bonded_validator '6' > /dev/null); then
        log "ERROR: validator $NODE_ID found in auction!"
        nctl-view-chain-auction
        exit 1
    else
        log "... validator $NODE_ID not found in auction [expected]"
    fi
}

# ----------------------------------------------------------------
# ENTRY POINT
# ----------------------------------------------------------------

unset STEP
STEP=0

main
