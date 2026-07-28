module tuned_device_rules_model
    implicit none
contains
    pure logical function selected(has_positive, positive_match, negative_match) result(accept)
        logical, intent(in) :: has_positive
        logical, intent(in) :: positive_match
        logical, intent(in) :: negative_match

        accept = (.not. negative_match) .and. (positive_match .or. (.not. has_positive))
    end function selected
end module tuned_device_rules_model

program tuned_device_rules_check
    use tuned_device_rules_model
    implicit none

    if (.not. selected(.true., .true., .false.)) then
        error stop "a positive match was rejected"
    end if
    if (selected(.true., .false., .false.)) then
        error stop "a non-matching positive rule was accepted"
    end if
    if (selected(.true., .true., .true.)) then
        error stop "a negative rule did not override a positive rule"
    end if
    if (.not. selected(.false., .false., .false.)) then
        error stop "negative-only rules did not receive an implicit match-all"
    end if
    if (selected(.false., .false., .true.)) then
        error stop "an implicit match-all bypassed a negative rule"
    end if
end program tuned_device_rules_check
