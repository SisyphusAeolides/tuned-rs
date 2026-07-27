module tuned_unit_model
    implicit none

    type :: option_state
        logical :: present = .false.
        integer :: value = 0
    end type option_state

contains

    pure function absent_option() result(state)
        type(option_state) :: state

        state%present = .false.
        state%value = 0
    end function absent_option

    pure function present_option(value) result(state)
        integer, intent(in) :: value
        type(option_state) :: state

        state%present = .true.
        state%value = value
    end function present_option

    pure function overlay_option(base, incoming, replace_unit, drop_option) result(state)
        type(option_state), intent(in) :: base
        type(option_state), intent(in) :: incoming
        logical, intent(in) :: replace_unit
        logical, intent(in) :: drop_option
        type(option_state) :: state

        if (replace_unit) then
            state = incoming
        else if (drop_option) then
            state = absent_option()
        else if (incoming%present) then
            state = incoming
        else
            state = base
        end if
    end function overlay_option

    pure logical function same_option(left, right) result(equal)
        type(option_state), intent(in) :: left
        type(option_state), intent(in) :: right

        equal = left%present .eqv. right%present
        if (equal .and. left%present) equal = left%value == right%value
    end function same_option

end module tuned_unit_model

program tuned_unit_check
    use tuned_unit_model
    implicit none

    type(option_state) :: base
    type(option_state) :: incoming
    type(option_state) :: result

    base = present_option(10)
    incoming = present_option(20)

    result = overlay_option(base, incoming, .false., .false.)
    if (.not. same_option(result, incoming)) then
        error stop "incoming option did not override the inherited option"
    end if

    result = overlay_option(base, absent_option(), .false., .false.)
    if (.not. same_option(result, base)) then
        error stop "an unspecified option did not preserve the inherited option"
    end if

    result = overlay_option(base, incoming, .false., .true.)
    if (result%present) then
        error stop "drop did not remove the inherited option"
    end if

    result = overlay_option(base, absent_option(), .true., .false.)
    if (result%present) then
        error stop "replace did not discard the inherited option set"
    end if

    result = overlay_option(base, incoming, .true., .true.)
    if (.not. same_option(result, incoming)) then
        error stop "replacement was incorrectly altered by drop metadata"
    end if
end program tuned_unit_check
