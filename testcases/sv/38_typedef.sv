module veryl_testcase_Module38;
    typedef logic  [16-1:0]         word_t   ;
    typedef logic  [16-1:0][16-1:0] words_t  ;
    typedef word_t                  regfile_t [0:2-1];

    typedef bit [8-1:0] octbyte [0:8-1];

    regfile_t rf   ;
    always_comb rf[0] = '0;
    always_comb rf[1] = '0;
endmodule

interface veryl_testcase_Interface38;
    typedef logic  [16-1:0]         word_t   ;
    typedef logic  [16-1:0][16-1:0] words_t  ;
    typedef word_t                  regfile_t [0:16-1];

    typedef bit [8-1:0] octbyte [0:8-1];
endinterface

package veryl_testcase_Package38;
    typedef logic  [16-1:0]         word_t   ;
    typedef logic  [16-1:0][16-1:0] words_t  ;
    typedef word_t                  regfile_t [0:16-1];

    typedef bit [8-1:0] octbyte [0:8-1];
endpackage
//# sourceMappingURL=../map/38_typedef.sv.map
