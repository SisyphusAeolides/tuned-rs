module TunedInstances

%default total

public export
data Plugin = Cpu | Disk

public export
Eq Plugin where
  Cpu == Cpu = True
  Disk == Disk = True
  _ == _ = False

public export
data Owner
  = Unowned
  | OwnedBy Plugin Nat

public export
data TransferResult
  = Accepted Owner
  | Rejected Owner

public export
acquire : Owner -> Plugin -> Nat -> TransferResult
acquire Unowned requested target = Rejected Unowned
acquire current@(OwnedBy existing owner) requested target =
  if existing == requested
     then Accepted (OwnedBy requested target)
     else Rejected current

public export
release : Owner -> Nat -> Owner
release Unowned target = Unowned
release current@(OwnedBy plugin owner) target =
  if owner == target then Unowned else current

public export
samePluginTransferIsExclusive :
  acquire (OwnedBy Cpu 1) Cpu 2 = Accepted (OwnedBy Cpu 2)
samePluginTransferIsExclusive = Refl

public export
crossPluginTransferPreservesOwner :
  acquire (OwnedBy Cpu 1) Disk 2 = Rejected (OwnedBy Cpu 1)
crossPluginTransferPreservesOwner = Refl

public export
unownedDeviceCannotBeAcquired :
  acquire Unowned Cpu 1 = Rejected Unowned
unownedDeviceCannotBeAcquired = Refl

public export
destroyedOwnerReleasesDevice :
  release (OwnedBy Cpu 3) 3 = Unowned
destroyedOwnerReleasesDevice = Refl

public export
destroyingAnotherInstancePreservesOwner :
  release (OwnedBy Cpu 3) 2 = OwnedBy Cpu 3
destroyingAnotherInstancePreservesOwner = Refl
