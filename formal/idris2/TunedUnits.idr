module TunedUnits

%default total

public export
data OptionState
  = Absent
  | Present Nat

public export
overlayOption :
  (replaceUnit : Bool) ->
  (dropOption : Bool) ->
  (base : OptionState) ->
  (incoming : OptionState) ->
  OptionState
overlayOption True _ _ incoming = incoming
overlayOption False True _ _ = Absent
overlayOption False False base Absent = base
overlayOption False False _ incoming@(Present _) = incoming

public export
incomingOverrides :
  overlayOption False False (Present 10) (Present 20) = Present 20
incomingOverrides = Refl

public export
unspecifiedPreservesBase :
  overlayOption False False (Present 10) Absent = Present 10
unspecifiedPreservesBase = Refl

public export
dropRemovesInherited :
  overlayOption False True (Present 10) (Present 20) = Absent
dropRemovesInherited = Refl

public export
replaceDiscardsInherited :
  overlayOption True False (Present 10) Absent = Absent
replaceDiscardsInherited = Refl

public export
replaceTakesPrecedenceOverDropMetadata :
  overlayOption True True (Present 10) (Present 20) = Present 20
replaceTakesPrecedenceOverDropMetadata = Refl
