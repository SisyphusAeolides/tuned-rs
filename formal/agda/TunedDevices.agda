{-# OPTIONS --safe #-}
module TunedDevices where

open import Agda.Builtin.Bool using (Bool; true; false)
open import Agda.Builtin.Equality using (_≡_; refl)

selected : Bool → Bool → Bool → Bool
selected _ _ true = false
selected true positiveMatch false = positiveMatch
selected false _ false = true

positiveMatchIsAccepted : selected true true false ≡ true
positiveMatchIsAccepted = refl

positiveMissIsRejected : selected true false false ≡ false
positiveMissIsRejected = refl

negativeRuleOverridesPositive : selected true true true ≡ false
negativeRuleOverridesPositive = refl

negativeOnlyRulesHaveImplicitMatchAll : selected false false false ≡ true
negativeOnlyRulesHaveImplicitMatchAll = refl

implicitMatchAllStillHonorsNegative : selected false false true ≡ false
implicitMatchAllStillHonorsNegative = refl
