    (component
        (core module $module
            (memory (export "memory") 1)
            (global $event-count (mut i32) (i32.const 0))

            (func (export "cabi_realloc")
                (param i32 i32 i32 i32)
                (result i32)
                (i32.const 8))

            (func (export "register")
                (global.set $event-count
                    (i32.add (global.get $event-count) (i32.const 1))))

            (func (export "start")
                (global.set $event-count
                    (i32.add (global.get $event-count) (i32.const 1))))

            (func (export "stop"))

            (func (export "on-event") (param i32 i32 i32 i32)
                (if (i32.eqz
                    (i32.or
                        (i32.eq (global.get $event-count) (i32.const 2))
                        (i32.eq (global.get $event-count) (i32.const 3))))
                    (then unreachable)))

            (func (export "handle-ui-action") (param i32 i32))

            (func (export "handle-service")
                (param i32 i32 i32 i32 i32)
                (result i32)
                (if (i32.eq (local.get 2) (i32.const 99))
                    (then unreachable))
                (i32.store (i32.const 64) (i32.const 0))
                (i32.store offset=4 (i32.const 64) (i32.const 0))
                (i32.const 64))
        )

        (core instance $instance (instantiate $module))
        (alias core export $instance "memory" (core memory $memory))
        (alias core export $instance "cabi_realloc" (core func $realloc))
        (alias core export $instance "register" (core func $register))
        (alias core export $instance "start" (core func $start))
        (alias core export $instance "stop" (core func $stop))
        (alias core export $instance "on-event" (core func $on-event))
        (alias core export $instance "handle-ui-action" (core func $handle-ui-action))
        (alias core export $instance "handle-service" (core func $handle-service))

        (type $lifecycle (func))
        (type $on-event-type (func (param "topic" string) (param "payload" (list u8))))
        (type $handle-ui-action-type (func (param "action-json" (list u8))))
        (type $handle-service-type
            (func
                (param "contract" string)
                (param "version" u32)
                (param "payload" (list u8))
                (result (list u8))))

        (func $register-lifted (type $lifecycle) (canon lift (core func $register)))
        (func $start-lifted (type $lifecycle) (canon lift (core func $start)))
        (func $stop-lifted (type $lifecycle) (canon lift (core func $stop)))
        (func $on-event-lifted (type $on-event-type)
            (canon lift (core func $on-event) (memory $memory) (realloc $realloc) string-encoding=utf8))
        (func $handle-ui-action-lifted (type $handle-ui-action-type)
            (canon lift
                (core func $handle-ui-action)
                (memory $memory)
                (realloc $realloc)))
        (func $handle-service-lifted (type $handle-service-type)
            (canon lift
                (core func $handle-service)
                (memory $memory)
                (realloc $realloc)
                string-encoding=utf8))

        (type $guest (instance
            (export "register" (func (type $lifecycle)))
            (export "start" (func (type $lifecycle)))
            (export "stop" (func (type $lifecycle)))
            (export "on-event" (func (type $on-event-type)))
            (export "handle-ui-action" (func (type $handle-ui-action-type)))
            (export "handle-service" (func (type $handle-service-type)))
        ))
        (component $guest-shim
            (type $lifecycle (func))
            (type $on-event-type (func (param "topic" string) (param "payload" (list u8))))
            (type $handle-ui-action-type (func (param "action-json" (list u8))))
            (type $handle-service-type
                (func
                    (param "contract" string)
                    (param "version" u32)
                    (param "payload" (list u8))
                    (result (list u8))))
            (import "register" (func $register (type $lifecycle)))
            (import "start" (func $start (type $lifecycle)))
            (import "stop" (func $stop (type $lifecycle)))
            (import "on-event" (func $on-event (type $on-event-type)))
            (import "handle-ui-action" (func $handle-ui-action (type $handle-ui-action-type)))
            (import "handle-service" (func $handle-service (type $handle-service-type)))
            (export "register" (func $register))
            (export "start" (func $start))
            (export "stop" (func $stop))
            (export "on-event" (func $on-event))
            (export "handle-ui-action" (func $handle-ui-action))
            (export "handle-service" (func $handle-service))
        )
        (instance $guest-instance (instantiate $guest-shim
            (with "register" (func $register-lifted))
            (with "start" (func $start-lifted))
            (with "stop" (func $stop-lifted))
            (with "on-event" (func $on-event-lifted))
            (with "handle-ui-action" (func $handle-ui-action-lifted))
            (with "handle-service" (func $handle-service-lifted))
        ))
        (export "rintawa:engine/guest@0.0.1" (instance $guest-instance))
    )
