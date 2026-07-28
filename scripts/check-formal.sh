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

fortran_module_dir="$build_dir/fortran-modules"
idris_source_dir="$build_dir/idris2"
agda_source_dir="$build_dir/agda"
mkdir -p "$fortran_module_dir" "$idris_source_dir" "$agda_source_dir"

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
        -J "$fortran_module_dir" \
        -I "$fortran_module_dir" \
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
    run_fortran_model \
        formal/fortran/tuned_devices.f90 \
        tuned-devices-check \
        "Fortran device selection model"
else
    echo "Fortran models: skipped (gfortran not found)" >&2
    missing=1
fi

if command -v idris2 >/dev/null 2>&1; then
    cp formal/idris2/*.idr "$idris_source_dir/"
    (
        cd "$idris_source_dir"
        idris2 --check TunedProfile.idr
        idris2 --check TunedInstances.idr
        idris2 --check TunedVerification.idr
        idris2 --check TunedUnits.idr
        idris2 --check TunedRuntime.idr
        idris2 --check TunedSysfs.idr
        idris2 --check TunedDevices.idr
    )
    echo "Idris profile, instance, verification, unit, runtime, sysfs, and device models: passed"
else
    echo "Idris models: skipped (idris2 not found)" >&2
    missing=1
fi

if command -v agda >/dev/null 2>&1; then
    cp formal/agda/*.agda "$agda_source_dir/"
    agda --safe -i "$agda_source_dir" "$agda_source_dir/TunedRollback.agda"
    agda --safe -i "$agda_source_dir" "$agda_source_dir/TunedInstances.agda"
    agda --safe -i "$agda_source_dir" "$agda_source_dir/TunedVerification.agda"
    agda --safe -i "$agda_source_dir" "$agda_source_dir/TunedUnits.agda"
    agda --safe -i "$agda_source_dir" "$agda_source_dir/TunedRuntime.agda"
    agda --safe -i "$agda_source_dir" "$agda_source_dir/TunedSysfs.agda"
    agda --safe -i "$agda_source_dir" "$agda_source_dir/TunedDevices.agda"
    echo "Agda rollback, instance, verification, unit, runtime, sysfs, and device proofs: passed"
else
    echo "Agda proofs: skipped (agda not found)" >&2
    missing=1
fi

if [ "$strict" -eq 1 ] && [ "$missing" -ne 0 ]; then
    echo "strict formal verification requires gfortran, idris2, and agda" >&2
    exit 1
fi
