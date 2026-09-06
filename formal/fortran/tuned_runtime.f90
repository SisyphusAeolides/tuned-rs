module tuned_runtime_model
    implicit none

    type :: runtime_unit
        integer :: priority = 0
        integer :: ordinal = 0
        logical :: enabled = .true.
        logical :: cpuinfo_matches = .true.
        logical :: uname_matches = .true.
    end type runtime_unit

contains

    pure logical function active(unit) result(is_active)
        type(runtime_unit), intent(in) :: unit

        is_active = unit%enabled .and. unit%cpuinfo_matches .and. unit%uname_matches
    end function active

    pure logical function ordered_before(left, right) result(before)
        type(runtime_unit), intent(in) :: left
        type(runtime_unit), intent(in) :: right

        before = left%priority < right%priority .or. &
            (left%priority == right%priority .and. left%ordinal < right%ordinal)
    end function ordered_before

    pure subroutine stable_sort(units)
        type(runtime_unit), intent(inout) :: units(:)
        type(runtime_unit) :: current
        integer :: index
        integer :: position

        do index = 2, size(units)
            current = units(index)
            position = index - 1
            do while (position >= 1)
                if (ordered_before(units(position), current)) exit
                units(position + 1) = units(position)
                position = position - 1
            end do
            units(position + 1) = current
        end do
    end subroutine stable_sort

end module tuned_runtime_model

program tuned_runtime_check
    use tuned_runtime_model
    implicit none

    type(runtime_unit) :: units(4)

    units(1) = runtime_unit(20, 0, .true., .true., .true.)
    units(2) = runtime_unit(10, 1, .true., .true., .true.)
    units(3) = runtime_unit(10, 2, .true., .true., .true.)
    units(4) = runtime_unit(0, 3, .true., .false., .true.)

    if (.not. active(units(1))) error stop "matching enabled unit was rejected"
    if (active(units(4))) error stop "cpuinfo mismatch was activated"

    call stable_sort(units(1:3))
    if (units(1)%ordinal /= 1) error stop "lower priority unit did not run first"
    if (units(2)%ordinal /= 2) error stop "equal priorities were not stable"
    if (units(3)%ordinal /= 0) error stop "higher priority unit ran too early"
end program tuned_runtime_check
