#!/bin/sh
set -eu

strict=0
if [ "${1:-}" = "--strict" ]; then
    strict=1
elif [ "$#" -ne 0 ]; then
    echo "usage: $0 [--strict]" >&2
    exit 2
fi

build_dir=$(mktemp -d)
trap 'rm -rf "$build_dir"' EXIT HUP INT TERM
missing=0

if command -v gfortran >/dev/null 2>&1; then
    gfortran \
        -std=f2008 \
        -ffree-line-length-none \
        -Wall \
        -Wextra \
        -Werror \
        formal/fortran/tuned_rollback.f90 \
        -o "$build_dir/tuned-rollback-check"
    "$build_dir/tuned-rollback-check"
    echo "Fortran rollback model: passed"
else
    echo "Fortran rollback model: skipped (gfortran not found)" >&2
    missing=1
fi

if command -v idris2 >/dev/null 2>&1; then
    idris2 --check formal/idris2/TunedProfile.idr
    echo "Idris profile model: passed"
else
    echo "Idris profile model: skipped (idris2 not found)" >&2
    missing=1
fi

if command -v agda >/dev/null 2>&1; then
    agda --safe -i formal/agda formal/agda/TunedRollback.agda
    echo "Agda rollback proof: passed"
else
    echo "Agda rollback proof: skipped (agda not found)" >&2
    missing=1
fi

if [ "$strict" -eq 1 ] && [ "$missing" -ne 0 ]; then
    echo "strict formal verification requires gfortran, idris2, and agda" >&2
    exit 1
fi
