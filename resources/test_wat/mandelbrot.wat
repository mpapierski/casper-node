;; INFO asc module.ts --textFile module.wat --outFile module.wasm --bindings raw -O3 --runtime stub
(module
 (type $i32_i32_i32_=>_none (func (param i32 i32 i32)))
 (memory $0 0)
 (export "mandelbrot" (func $module/mandelbrot))
 (export "memory" (memory $0))
 (func $module/mandelbrot (param $0 i32) (param $1 i32) (param $2 i32)
  (local $3 f64)
  (local $4 f64)
  (local $5 i32)
  (local $6 f64)
  (local $7 f64)
  (local $8 f64)
  (local $9 f64)
  (local $10 f64)
  (local $11 i32)
  (local $12 i32)
  (local $13 f64)
  (local $14 i32)
  (local $15 f64)
  local.get $1
  f64.convert_i32_u
  f64.const 0.5
  f64.mul
  local.set $7
  local.get $0
  f64.convert_i32_u
  f64.const 0.625
  f64.mul
  f64.const 10
  local.get $0
  i32.const 3
  i32.mul
  local.tee $5
  local.get $1
  i32.const 2
  i32.shl
  local.tee $12
  local.get $5
  local.get $12
  i32.lt_s
  select
  f64.convert_i32_s
  f64.div
  local.tee $10
  f64.mul
  local.set $6
  i32.const 8
  local.get $2
  local.get $2
  i32.const 8
  i32.gt_u
  select
  local.set $14
  loop $for-loop|0
   local.get $1
   local.get $11
   i32.gt_u
   if
    local.get $11
    f64.convert_i32_u
    local.get $7
    f64.sub
    local.get $10
    f64.mul
    local.set $9
    i32.const 0
    local.set $12
    loop $for-loop|1
     local.get $0
     local.get $12
     i32.gt_u
     if
      local.get $12
      f64.convert_i32_u
      local.get $10
      f64.mul
      local.get $6
      f64.sub
      local.set $8
      f64.const 0
      local.set $3
      f64.const 0
      local.set $13
      i32.const 0
      local.set $5
      loop $while-continue|2
       local.get $3
       local.get $3
       f64.mul
       local.tee $4
       local.get $13
       local.get $13
       f64.mul
       local.tee $15
       f64.add
       f64.const 4
       f64.le
       if
        block $while-break|2
         local.get $3
         local.get $3
         f64.add
         local.get $13
         f64.mul
         local.get $9
         f64.add
         local.set $13
         local.get $4
         local.get $15
         f64.sub
         local.get $8
         f64.add
         local.set $3
         local.get $2
         local.get $5
         i32.le_u
         br_if $while-break|2
         local.get $5
         i32.const 1
         i32.add
         local.set $5
         br $while-continue|2
        end
       end
      end
      loop $while-continue|3
       local.get $5
       local.get $14
       i32.lt_u
       if
        local.get $3
        local.get $3
        f64.mul
        local.get $13
        local.get $13
        f64.mul
        f64.sub
        local.get $8
        f64.add
        local.set $4
        local.get $3
        local.get $3
        f64.add
        local.get $13
        f64.mul
        local.get $9
        f64.add
        local.set $13
        local.get $4
        local.set $3
        local.get $5
        i32.const 1
        i32.add
        local.set $5
        br $while-continue|3
       end
      end
      local.get $12
      i32.const 1
      i32.add
      local.set $12
      br $for-loop|1
     end
    end
    local.get $11
    i32.const 1
    i32.add
    local.set $11
    br $for-loop|0
   end
  end
 )
)
