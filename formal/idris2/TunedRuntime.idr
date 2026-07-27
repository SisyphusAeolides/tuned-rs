module TunedRuntime

%default total

public export
record RuntimeUnit where
  constructor Unit
  priority : Nat
  ordinal : Nat
  enabled : Bool
  cpuinfoMatches : Bool
  unameMatches : Bool

public export
active : RuntimeUnit -> Bool
active unit = enabled unit && cpuinfoMatches unit && unameMatches unit

public export
orderedBefore : RuntimeUnit -> RuntimeUnit -> Bool
orderedBefore left right =
  if priority left < priority right then True
  else if priority right < priority left then False
  else ordinal left < ordinal right

public export
matchingEnabledUnit : RuntimeUnit
matchingEnabledUnit = Unit 10 0 True True True

public export
cpuinfoMismatch : RuntimeUnit
cpuinfoMismatch = Unit 0 1 True False True

public export
laterEqualPriority : RuntimeUnit
laterEqualPriority = Unit 10 2 True True True

public export
matchingUnitActivates : active matchingEnabledUnit = True
matchingUnitActivates = Refl

public export
cpuinfoMismatchCannotActivate : active cpuinfoMismatch = False
cpuinfoMismatchCannotActivate = Refl

public export
lowerPriorityRunsFirst :
  orderedBefore (Unit 5 3 True True True) matchingEnabledUnit = True
lowerPriorityRunsFirst = Refl

public export
equalPriorityKeepsDeclarationOrder :
  orderedBefore matchingEnabledUnit laterEqualPriority = True
equalPriorityKeepsDeclarationOrder = Refl
