

package veryl_testcase___BasePkg81__8__32;
    typedef enum logic [1-1:0] {
        Command_WRITE,
        Command_READ
    } Command;

    typedef logic [8-1:0]  Address;
    typedef logic [32-1:0] Data   ;
endpackage


interface veryl_testcase___Interface81____BasePkg81__8__32;
    logic                                      ready  ;
    logic                                      valid  ;
    veryl_testcase___BasePkg81__8__32::Command command;
    veryl_testcase___BasePkg81__8__32::Address address;
    veryl_testcase___BasePkg81__8__32::Data    data   ;

    modport master (
        input  ready  ,
        output valid  ,
        output command,
        output address,
        output data   
    );

    modport slave (
        output ready  ,
        input  valid  ,
        input  command,
        input  address,
        input  data   
    );
endinterface

module veryl_testcase_Module81A (
    input  var logic                                      __m_if_0_0_ready  ,
    output var logic                                      __m_if_0_0_valid  ,
    output var veryl_testcase___BasePkg81__8__32::Command __m_if_0_0_command,
    output var veryl_testcase___BasePkg81__8__32::Address __m_if_0_0_address,
    output var veryl_testcase___BasePkg81__8__32::Data    __m_if_0_0_data   ,
    input  var logic                                      __m_if_0_1_ready  ,
    output var logic                                      __m_if_0_1_valid  ,
    output var veryl_testcase___BasePkg81__8__32::Command __m_if_0_1_command,
    output var veryl_testcase___BasePkg81__8__32::Address __m_if_0_1_address,
    output var veryl_testcase___BasePkg81__8__32::Data    __m_if_0_1_data   ,
    output var logic                                      __s_if_0_0_ready  ,
    input  var logic                                      __s_if_0_0_valid  ,
    input  var veryl_testcase___BasePkg81__8__32::Command __s_if_0_0_command,
    input  var veryl_testcase___BasePkg81__8__32::Address __s_if_0_0_address,
    input  var veryl_testcase___BasePkg81__8__32::Data    __s_if_0_0_data   ,
    output var logic                                      __s_if_0_1_ready  ,
    input  var logic                                      __s_if_0_1_valid  ,
    input  var veryl_testcase___BasePkg81__8__32::Command __s_if_0_1_command,
    input  var veryl_testcase___BasePkg81__8__32::Address __s_if_0_1_address,
    input  var veryl_testcase___BasePkg81__8__32::Data    __s_if_0_1_data   
);
    veryl_testcase___Interface81____BasePkg81__8__32 m_if [0:1-1][0:2-1] ();
    always_comb begin
        m_if[0][0].ready   = __m_if_0_0_ready  ;
        __m_if_0_0_valid   = m_if[0][0].valid  ;
        __m_if_0_0_command = m_if[0][0].command;
        __m_if_0_0_address = m_if[0][0].address;
        __m_if_0_0_data    = m_if[0][0].data   ;
    end
    always_comb begin
        m_if[0][1].ready   = __m_if_0_1_ready  ;
        __m_if_0_1_valid   = m_if[0][1].valid  ;
        __m_if_0_1_command = m_if[0][1].command;
        __m_if_0_1_address = m_if[0][1].address;
        __m_if_0_1_data    = m_if[0][1].data   ;
    end
    veryl_testcase___Interface81____BasePkg81__8__32 s_if [0:1-1][0:2-1] ();
    always_comb begin
        __s_if_0_0_ready   = s_if[0][0].ready  ;
        s_if[0][0].valid   = __s_if_0_0_valid  ;
        s_if[0][0].command = __s_if_0_0_command;
        s_if[0][0].address = __s_if_0_0_address;
        s_if[0][0].data    = __s_if_0_0_data   ;
    end
    always_comb begin
        __s_if_0_1_ready   = s_if[0][1].ready  ;
        s_if[0][1].valid   = __s_if_0_1_valid  ;
        s_if[0][1].command = __s_if_0_1_command;
        s_if[0][1].address = __s_if_0_1_address;
        s_if[0][1].data    = __s_if_0_1_data   ;
    end
    for (genvar i = 0; i < 1; i++) begin :g
        for (genvar j = 0; j < 2; j++) begin :g
            always_comb begin
                s_if[i][j].ready   = m_if[i][j].ready;
                m_if[i][j].valid   = s_if[i][j].valid;
                m_if[i][j].command = s_if[i][j].command;
                m_if[i][j].address = s_if[i][j].address;
                m_if[i][j].data    = s_if[i][j].data;
            end
        end
    end
endmodule

module veryl_testcase_Module81B;
    veryl_testcase___Interface81____BasePkg81__8__32 a_if [0:1-1][0:2-1] ();
    veryl_testcase___Interface81____BasePkg81__8__32 b_if [0:1-1][0:2-1] ();

    for (genvar i = 0; i < 1; i++) begin :g
        for (genvar j = 0; j < 2; j++) begin :g
            always_comb begin
                a_if[i][j].ready = 0;
            end
            always_comb begin
                b_if[i][j].valid   = 0;
                b_if[i][j].command = veryl_testcase___BasePkg81__8__32::Command'(0);
                b_if[i][j].address = veryl_testcase___BasePkg81__8__32::Address'(0);
                b_if[i][j].data    = veryl_testcase___BasePkg81__8__32::Data'(0);
            end
        end
    end

    veryl_testcase_Module81A u (
        .__m_if_0_0_ready   (a_if[0][0].ready  ),
        .__m_if_0_0_valid   (a_if[0][0].valid  ),
        .__m_if_0_0_command (a_if[0][0].command),
        .__m_if_0_0_address (a_if[0][0].address),
        .__m_if_0_0_data    (a_if[0][0].data   ),
        .__m_if_0_1_ready   (a_if[0][1].ready  ),
        .__m_if_0_1_valid   (a_if[0][1].valid  ),
        .__m_if_0_1_command (a_if[0][1].command),
        .__m_if_0_1_address (a_if[0][1].address),
        .__m_if_0_1_data    (a_if[0][1].data   ),
        .__s_if_0_0_ready   (b_if[0][0].ready  ),
        .__s_if_0_0_valid   (b_if[0][0].valid  ),
        .__s_if_0_0_command (b_if[0][0].command),
        .__s_if_0_0_address (b_if[0][0].address),
        .__s_if_0_0_data    (b_if[0][0].data   ),
        .__s_if_0_1_ready   (b_if[0][1].ready  ),
        .__s_if_0_1_valid   (b_if[0][1].valid  ),
        .__s_if_0_1_command (b_if[0][1].command),
        .__s_if_0_1_address (b_if[0][1].address),
        .__s_if_0_1_data    (b_if[0][1].data   )
    );
endmodule
//# sourceMappingURL=../map/81_modport_expansion.sv.map
