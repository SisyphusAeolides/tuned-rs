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

run_fortran_model() {
    source=$1
    output=$2
    description=$3
    gfortran \
        -std=f2008 \
        -ffree-line-length-none \
        -Wall \
        -Wextra \
        -Werror \
        "$source" \
        -o "$build_dir/$output"
    "$build_dir/$output"
    echo "$description: passed"
}

if command -v gfortran >/dev/null 2>&1; then
    run_fortran_model \
        formal/fortran/tuned_rollback.f90 \
        tuned-rollback-check \
        "Fortran rollback model"
    run_fortran_model \
        formal/fortran/tuned_instances.f90 \
        tuned-instances-check \
        "Fortran instance ownership model"
    run_fortran_model \
        formal/fortran/tuned_verification.f90 \
        tuned-verification-check \
        "Fortran verification policy model"
    run_fortran_model \
        formal/fortran/tuned_units.f90 \
        tuned-units-check \
        "Fortran profile unit overlay model"
    run_fortran_model \
        formal/fortran/tuned_runtime.f90 \
        tuned-runtime-check \
        "Fortran runtime unit selection model"
    run_fortran_model \
        formal/fortran/tuned_sysfs.f90 \
        tuned-sysfs-check \
        "Fortran generic sysfs safety model"
else
    echo "Fortran models: skipped (gfortran not found)" >&2
    missing=1
fi

if command -v idris2 >/dev/null 2>&1; then
    (
        cd formal/idris2
        idris2 --check TunedProfile.idr
        idris2 --check TunedInstances.idr
        idris2 --check TunedVerification.idr
        idris2 --check TunedUnits.idr
        idris2 --check TunedRuntime.idr
        idris2 --check TunedSysfs.idr
    )
    echo "Idris profile, instance, verification, unit, runtime, and sysfs models: passed"
else
    echo "Idris models: skipped (idris2 not found)" >&2
    missing=1
fi

if command -v agda >/dev/null 2>&1; then
    agda --safe -i formal/agda formal/agda/TunedRollback.agda
    agda --safe -i formal/agda formal/agda/TunedInstances.agda
    agda --safe -i formal/agda formal/agda/TunedVerification.agda
    agda --safe -i formal/agda formal/agda/TunedUnits.agda
    agda --safe -i formal/agda formal/agda/TunedRuntime.agda
    agda --safe -i formal/agda formal/agda/TunedSysfs.agda
    echo "Agda rollback, instance, verification, unit, runtime, and sysfs proofs: passed"
else
    echo "Agda proofs: skipped (agda not found)" >&2
    missing=1
fi

if [ "$strict" -eq 1 ] && [ "$missing" -ne 0 ]; then
    echo "strict formal verification requires gfortran, idris2, and agda" >&2
    exit 1
fi
