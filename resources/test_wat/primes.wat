(module
 (memory $0 0)
 (export "countPrimes" (func $module/countPrimes))
 (export "memory" (memory $0))
 (func $module/countPrimes (param $0 i64) (param $1 i64) (result i64)
  (local $2 i32)
  loop $for-loop|0
   local.get $0
   local.get $1
   i64.lt_s
   if
    local.get $2
    i32.const 1
    i32.add
    local.get $2
    block $__inlined_func$module/isPrime (result i32)
     i32.const 0
     local.get $0
     i64.const 2
     i64.lt_s
     br_if $__inlined_func$module/isPrime
     drop
     local.get $0
     i64.const 2
     i64.eq
     local.get $0
     i64.const 1
     i64.and
     i64.eqz
     br_if $__inlined_func$module/isPrime
     drop
     local.get $0
     i64.const 3
     i64.eq
     local.get $0
     i64.const 3
     i64.rem_s
     i64.eqz
     br_if $__inlined_func$module/isPrime
     drop
     i32.const 5
     local.set $2
     loop $while-continue|0
      local.get $2
      local.get $2
      i32.mul
      i64.extend_i32_s
      local.get $0
      i64.le_s
      if
       i32.const 0
       local.get $0
       local.get $2
       i64.extend_i32_s
       i64.rem_s
       i64.eqz
       br_if $__inlined_func$module/isPrime
       drop
       i32.const 0
       local.get $0
       local.get $2
       i32.const 2
       i32.add
       local.tee $2
       i64.extend_i32_s
       i64.rem_s
       i64.eqz
       br_if $__inlined_func$module/isPrime
       drop
       local.get $2
       i32.const 4
       i32.add
       local.set $2
       br $while-continue|0
      end
     end
     i32.const 1
    end
    select
    local.set $2
    local.get $0
    i64.const 1
    i64.add
    local.set $0
    br $for-loop|0
   end
  end
  local.get $2
  i64.extend_i32_s
 )
)
