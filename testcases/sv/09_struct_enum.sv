module Module09 ;
    // struct declaration
    typedef struct {
        logic        [10-1:0] a  ;
        logic        [10-1:0] aa ;
        int unsigned         aaa ;
    } A;

    // enum declaration
    typedef enum logic [2-1:0] {
        X = 1,
        Y = 2,
        Z
    } B;
endmodule
