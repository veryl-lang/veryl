module veryl_testcase_Module38;
    typedef logic        [16-1:0]         word_t   ;
    typedef logic        [16-1:0][16-1:0] words_t  ;
    typedef word_t                         regfile_t [0:16-1];

    typedef bit     [8-1:0] octbyte [0:8-1];

    regfile_t rf   ;
    assign rf[0] = '0;
endmodule
