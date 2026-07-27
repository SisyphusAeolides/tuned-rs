{-# OPTIONS --safe #-}
module TunedVerification where

open import Agda.Builtin.Bool using (Bool; true; false; _&&_)
open import Agda.Builtin.Equality using (_≡_; refl)
open import Agda.Builtin.List using (List; []; _∷_)

data Verdict : Set where
  match missing mismatch unsupported readError : Verdict

issuePasses : Bool → Verdict → Bool
issuePasses _ match = true
issuePasses true missing = true
issuePasses false missing = false
issuePasses _ mismatch = false
issuePasses _ unsupported = false
issuePasses _ readError = false

reportPasses : Bool → List Verdict → Bool
reportPasses ignoreMissing [] = true
reportPasses ignoreMissing (verdict ∷ rest) =
  issuePasses ignoreMissing verdict && reportPasses ignoreMissing rest

strictMissingFails : issuePasses false missing ≡ false
strictMissingFails = refl

ignoredMissingPasses : issuePasses true missing ≡ true
ignoredMissingPasses = refl

mismatchCannotBeWaived : issuePasses true mismatch ≡ false
mismatchCannotBeWaived = refl

unsupportedCannotBeWaived : issuePasses true unsupported ≡ false
unsupportedCannotBeWaived = refl

readErrorCannotBeWaived : issuePasses true readError ≡ false
readErrorCannotBeWaived = refl

missingOnlyReportMayPass :
  reportPasses true (match ∷ missing ∷ match ∷ []) ≡ true
missingOnlyReportMayPass = refl

mismatchReportAlwaysFails :
  reportPasses true (match ∷ mismatch ∷ missing ∷ []) ≡ false
mismatchReportAlwaysFails = refl
