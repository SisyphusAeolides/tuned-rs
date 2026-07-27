program tuned_instances
    implicit none

    integer, parameter :: free_owner = 0
    integer, parameter :: cpu_plugin = 1
    integer, parameter :: disk_plugin = 2
    integer :: owners(4)
    integer :: plugins(3)
    integer :: snapshot(4)
    logical :: ok

    plugins = [cpu_plugin, cpu_plugin, disk_plugin]
    owners = [1, 1, 3, free_owner]

    call assert_registry(owners, size(plugins))

    call acquire_devices(owners, plugins, [2], 2, ok)
    call require(ok, "same-plugin transfer must succeed")
    call require(all(owners == [1, 2, 3, free_owner]), &
        "same-plugin transfer must move exactly one device")
    call assert_registry(owners, size(plugins))

    snapshot = owners
    call acquire_devices(owners, plugins, [3], 2, ok)
    call require(.not. ok, "cross-plugin transfer must fail")
    call require(all(owners == snapshot), &
        "failed cross-plugin transfer must be transactional")

    snapshot = owners
    call acquire_devices(owners, plugins, [4], 2, ok)
    call require(.not. ok, "unowned device acquisition must fail")
    call require(all(owners == snapshot), &
        "failed unowned-device acquisition must be transactional")

    call destroy_instance(owners, 1)
    call require(owners(1) == free_owner, &
        "destroyed instance must release its devices")
    call require(owners(2) == 2 .and. owners(3) == 3, &
        "destroy must not disturb other owners")
    call assert_registry(owners, size(plugins))

contains

    subroutine acquire_devices(owner_table, plugin_table, devices, target, success)
        integer, intent(inout) :: owner_table(:)
        integer, intent(in) :: plugin_table(:)
        integer, intent(in) :: devices(:)
        integer, intent(in) :: target
        logical, intent(out) :: success
        integer :: before(size(owner_table))
        integer :: index
        integer :: device
        integer :: current_owner

        before = owner_table
        success = .false.

        if (target < 1 .or. target > size(plugin_table)) return

        do index = 1, size(devices)
            device = devices(index)
            if (device < 1 .or. device > size(owner_table)) then
                owner_table = before
                return
            end if

            current_owner = owner_table(device)
            if (current_owner == free_owner) then
                owner_table = before
                return
            end if
            if (current_owner < 1 .or. current_owner > size(plugin_table)) then
                owner_table = before
                return
            end if
            if (plugin_table(current_owner) /= plugin_table(target)) then
                owner_table = before
                return
            end if
        end do

        do index = 1, size(devices)
            owner_table(devices(index)) = target
        end do
        success = .true.
    end subroutine acquire_devices

    subroutine destroy_instance(owner_table, instance)
        integer, intent(inout) :: owner_table(:)
        integer, intent(in) :: instance
        integer :: index

        do index = 1, size(owner_table)
            if (owner_table(index) == instance) owner_table(index) = free_owner
        end do
    end subroutine destroy_instance

    subroutine assert_registry(owner_table, instance_total)
        integer, intent(in) :: owner_table(:)
        integer, intent(in) :: instance_total
        integer :: index

        do index = 1, size(owner_table)
            call require(owner_table(index) >= free_owner, &
                "owner identifier must not be negative")
            call require(owner_table(index) <= instance_total, &
                "owner identifier must name a live instance")
        end do
    end subroutine assert_registry

    subroutine require(condition, message)
        logical, intent(in) :: condition
        character(len=*), intent(in) :: message

        if (.not. condition) then
            write (*, '(A)') trim(message)
            error stop 1
        end if
    end subroutine require

end program tuned_instances
