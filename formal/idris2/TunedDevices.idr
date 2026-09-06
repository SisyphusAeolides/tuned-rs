module TunedDevices

%default total

public export
selected :
  (hasPositive : Bool) ->
  (positiveMatch : Bool) ->
  (negativeMatch : Bool) ->
  Bool
selected _ _ True = False
selected True positiveMatch False = positiveMatch
selected False _ False = True

public export
positiveMatchIsAccepted : selected True True False = True
positiveMatchIsAccepted = Refl

public export
positiveMissIsRejected : selected True False False = False
positiveMissIsRejected = Refl

public export
negativeRuleOverridesPositive : selected True True True = False
negativeRuleOverridesPositive = Refl

public export
negativeOnlyRulesHaveImplicitMatchAll : selected False False False = True
negativeOnlyRulesHaveImplicitMatchAll = Refl

public export
implicitMatchAllStillHonorsNegative : selected False False True = False
implicitMatchAllStillHonorsNegative = Refl
