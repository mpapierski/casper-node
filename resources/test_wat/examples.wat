(module

  (memory $memory 1)
    (func $fib_recursive (export "fibonacci_rec") (param $N i64) (result i64)
        (if
            (i64.le_s (local.get $N) (i64.const 1))
            (then (return (local.get $N)))
        )
        (return
            (i64.add
                (call $fib_recursive
                  (i64.sub (local.get $N) (i64.const 1))
                )
                (call $fib_recursive
                  (i64.sub (local.get $N) (i64.const 2))
                )
            )
        )
    )

    (func $fib_iterative (export "fibonacci_iter") (param $N i64) (result i64)
        (local $n1 i64)
        (local $n2 i64)
        (local $tmp i64)
        (local $i i64)
        ;; return $N for N <= 1
        (if
            (i64.le_s (local.get $N) (i64.const 1))
            (then (return (local.get $N)))
        )
        (local.set $n1 (i64.const 1))
        (local.set $n2 (i64.const 1))
        (local.set $i (i64.const 2))
        ;;since we normally return n2, handle n=1 case specially
        (loop $continue
            (if
                (i64.lt_s (local.get $i) (local.get $N))
                (then
                    (local.set $tmp (i64.add (local.get $n1) (local.get $n2)))
                    (local.set $n1 (local.get $n2))
                    (local.set $n2 (local.get $tmp))
                    (local.set $i (i64.add (local.get $i) (i64.const 1)))
                    (br $continue)
                )
            )
        )
        (local.get $n2)
    )

    (func $cpu_burner (export "infinite_loop")
	  (loop $continue
	  (loop $continue
	  (loop $continue
	  (loop $continue
		(nop)
		(br $continue)))))
      	  )
 (func $br_if (export "br_if") (param i64)
    (local i64)
    i64.const 0
    local.set 1
    loop  ;; label = @1
      local.get 1
      i64.const 1
      i64.add
      local.tee 1
      local.get 0
      i64.lt_u
       nop
      br_if 0 (;@1;)
    end)

  (func $br_table (export "br_table") (param i64)
    (local i64)
    i64.const 0
    local.set 1
    block  ;; label = @1
      loop  ;; label = @2
        block  ;; label = @3
          local.get 1
          i64.const 1
          i64.add
          local.tee 1
          local.get 0
          i64.lt_u
          br_table 2 (;@1;) 1 (;@2;) 0 (;@3;)
        end
        unreachable
      end
    end)

 (func $ackermann (export "ackermann") (param i64 i64) (result i64)
    block  ;; label = @1
      local.get 0
      i64.eqz
      br_if 0 (;@1;)
      loop  ;; label = @2
        block  ;; label = @3
          block  ;; label = @4
            local.get 1
            i64.const 0
            i64.ne
            br_if 0 (;@4;)
            i64.const 1
            local.set 1
            br 1 (;@3;)
          end
          local.get 0
          local.get 1
          i64.const -1
          i64.add
          call $ackermann
          local.set 1
        end
        local.get 0
        i64.const -1
        i64.add
        local.tee 0
        i64.eqz
        i32.eqz
        br_if 0 (;@2;)
      end
    end
    local.get 1
    i64.const 1
    i64.add)


  (func $fac (export "factorial")
    (param $n i64)
    (result i64)
    local.get $n
    i64.const 2
    i64.le_u

    if (result i64)
      local.get $n
    else
      local.get $n
      local.get $n
      i64.const 1
      i64.sub
      call $fac
      i64.mul
    end)

    (func $recursive_ok (export "recursive_ok") (param $n i32) (result i32)
        (if (result i32)
            (local.get $n)
            (then
                (return
                    (call $recursive_ok
                        (i32.sub
                            (local.get $n)
                            (i32.const 1)
                        )
                    )
                )
            )
            (else
                (return (local.get $n))
            )
        )
	)

(func $recursive_scan (export "recursive_scan") (param $n i32) (result i32)
        (if (result i32)
            (i32.eq (local.get $n) (i32.const 0))
            (then
                ;; return 0 if $n == 0
                (i32.const 0)
            )
            (else
                ;; return $n + (call $recursive_scanc($n - 1)) otherwise
                (i32.add
                    (call $recursive_scan
                        (i32.sub
                            (local.get $n)
                            (i32.const 1)
                        )
                    )
                    (local.get $n)
                )
            )
        )
    )

 (func (export "vec_add")
        (param $ptr_result i32)
        (param $ptr_a i32)
        (param $ptr_b i32)
        (param $len i32)
        (local $n i32)
        (block $exit
            (loop $loop
                (br_if ;; exit loop if $n == $len
                    $exit
                    (i32.eq
                        (local.get $n)
                        (local.get $len)
                    )
                )
                (i64.store offset=0 ;; ptr_result[n] = ptr_a[n] + ptr_b[n]
                    (i32.add
                        (local.get $ptr_result)
                        (i32.mul
                            (local.get $n)
                            (i32.const 8)
                        )
                    )
                    (i64.add
                        (i64.load32_s offset=0 ;; load ptr_a[n]
                            (i32.add
                                (local.get $ptr_a)
                                (i32.mul
                                    (local.get $n)
                                    (i32.const 4)
                                )
                            )
                        )
                        (i64.load32_s offset=0 ;; load ptr_b[n]
                            (i32.add
                                (local.get $ptr_b)
                                (i32.mul
                                    (local.get $n)
                                    (i32.const 4)
                                )
                            )
                        )
                    )
                )
                (local.set $n ;; increment n
                    (i32.add (local.get $n) (i32.const 1))
                 )
                (br $loop) ;; continue loop
            )
        )
        (return)
    )

    (func (export "fill_bytes") (param $ptr i32) (param $len i32) (param $value i32)
        (local $n i32)
        (block $exit
            (loop $loop
                (br_if ;; exit loop if $n == $len
                    $exit
                    (i32.eq
                        (local.get $n)
                        (local.get $len)
                    )
                )
                (i32.store8 offset=0 ;; store $value at mem[ptr+n]
                    (i32.add
                        (local.get $ptr)
                        (local.get $n)
                    )
                    (local.get $value)
                )
                (local.set $n ;; increment n
                    (i32.add (local.get $n) (i32.const 1))
                 )
                (br $loop) ;; continue loop
            )
        )
        (return)
    )
)
