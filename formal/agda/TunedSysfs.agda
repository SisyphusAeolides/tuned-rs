{-# OPTIONS --safe #-}
module TunedSysfs where

open import Agda.Builtin.Bool using (Bool; true; false)
open import Agda.Builtin.Equality using (_≡_; refl)

data Zone : Set where
  insideSysfs outsideSysfs : Zone

data ControlState : Set where
  missing present : ControlState

data ApplyResult : Set where
  rejected applied : ApplyResult

data RollbackResult : Set where
  untouched restored : RollbackResult

applyControl : Zone → ControlState → Bool → ApplyResult
applyControl insideSysfs present true = applied
applyControl _ _ _ = rejected

rollbackControl : ApplyResult → Bool → RollbackResult
rollbackControl applied true = restored
rollbackControl _ _ = untouched

outsidePathIsRejected :
  applyControl outsideSysfs present true ≡ rejected
outsidePathIsRejected = refl

missingControlIsRejected :
  applyControl insideSysfs missing true ≡ rejected
missingControlIsRejected = refl

unreadableControlIsRejected :
  applyControl insideSysfs present false ≡ rejected
unreadableControlIsRejected = refl

validControlIsApplied :
  applyControl insideSysfs present true ≡ applied
validControlIsApplied = refl

recordedApplyCanBeRestored :
  rollbackControl applied true ≡ restored
recordedApplyCanBeRestored = refl

missingSnapshotCannotInventRestore :
  rollbackControl applied false ≡ untouched
missingSnapshotCannotInventRestore = refl
