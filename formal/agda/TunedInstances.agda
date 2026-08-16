{-# OPTIONS --safe #-}
module TunedInstances where

open import Agda.Builtin.Equality using (_≡_; refl)
open import Agda.Builtin.Nat using (Nat; zero; suc)


data Plugin : Set where
  cpu disk : Plugin


data Owner : Set where
  unowned : Owner
  ownedBy : Plugin → Nat → Owner


data TransferResult : Set where
  accepted rejected : Owner → TransferResult


acquire : Owner → Plugin → Nat → TransferResult
acquire unowned requested target = rejected unowned
acquire (ownedBy cpu current) cpu target = accepted (ownedBy cpu target)
acquire current@(ownedBy cpu owner) disk target = rejected current
acquire current@(ownedBy disk owner) cpu target = rejected current
acquire (ownedBy disk current) disk target = accepted (ownedBy disk target)


releaseCurrent : Owner → Owner
releaseCurrent unowned = unowned
releaseCurrent (ownedBy plugin owner) = unowned


samePluginTransferIsExclusive :
  acquire (ownedBy cpu zero) cpu (suc zero)
    ≡ accepted (ownedBy cpu (suc zero))
samePluginTransferIsExclusive = refl


crossPluginTransferPreservesOwner :
  acquire (ownedBy cpu zero) disk (suc zero)
    ≡ rejected (ownedBy cpu zero)
crossPluginTransferPreservesOwner = refl


unownedDeviceCannotBeAcquired :
  acquire unowned cpu zero ≡ rejected unowned
unownedDeviceCannotBeAcquired = refl


destroyedOwnerReleasesDevice :
  releaseCurrent (ownedBy disk (suc zero)) ≡ unowned
destroyedOwnerReleasesDevice = refl
