#!/usr/bin/env bash

export CARGO_BUILD_TARGET="${TARGET}"
export CARGO_TARGET_DIR=target/build/"${CARGO_BUILD_TARGET}"
cross "${@}"