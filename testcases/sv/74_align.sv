module veryl_testcase_Module74;
    logic [32-1:0] a  ; always_comb a   = 1;
    logic [32-1:0] aa ; always_comb aa  = 1;
    logic [32-1:0] aaa; always_comb aaa = 1;

    logic _b; always_comb _b = {
        {1{a[0]}}, {1{a[0]}},
        {8{aa[1]}}, {8{aa[1]}},
        {16{aaa[2]}}, {16{aaa[2]}},
        {1{a[3]}}, {1{a[3]}},
        {8{aa[4]}}, {8{aa[4]}},
        {16{aaa[5]}}, {16{aaa[5]}},
        {1{a[6]}}, {1{a[6]}},
        {8{aa[7]}}, {8{aa[7]}},
        {16{aaa[8]}}, {16{aaa[8]}},
        {1{a[9]}}, {1{a[9]}},
        {8{aa[10]}}, {8{aa[10]}},
        {16{aaa[11]}}, {16{aaa[11]}}
    };

    logic _c ; always_comb _c  = {
        {1 {a  [0 ]}}, {1 {a  [0 ]}},
        {8 {aa [1 ]}}, {8 {aa [1 ]}},
        {16{aaa[2 ]}}, {16{aaa[2 ]}},
        {1 {a  [3 ]}}, {1 {a  [3 ]}},
        {8 {aa [4 ]}}, {8 {aa [4 ]}},
        {16{aaa[5 ]}}, {16{aaa[5 ]}},
        {1 {a  [6 ]}}, {1 {a  [6 ]}},
        {8 {aa [7 ]}}, {8 {aa [7 ]}},
        {16{aaa[8 ]}}, {16{aaa[8 ]}},
        {1 {a  [9 ]}}, {1 {a  [9 ]}},
        {8 {aa [10]}}, {8 {aa [10]}},
        {16{aaa[11]}}, {16{aaa[11]}}
    };
endmodule
//# sourceMappingURL=../map/testcases/sv/74_align.sv.map
