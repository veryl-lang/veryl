package veryl_testcase_Package80;
    typedef enum logic [1-1:0] {
        Command_WRITE,
        Command_READ
    } Command;

    typedef enum logic [1-1:0] {
        Status_OK,
        Status_ERROR
    } Status;
endpackage

interface veryl_testcase_Interface80A
    import veryl_testcase_Package80::Command;
;


    logic   command_ready;
    logic   command_valid;
    Command command      ;

    modport mp (
        output command_ready,
        input  command_valid,
        input  command      
    );
endinterface

interface veryl_testcase_Interface80B
    import veryl_testcase_Package80::Status;
;


    logic  status_ready;
    logic  status_valid;
    Status status      ;

    modport mp (
        input  status_ready,
        output status_valid,
        output status      
    );
endinterface

interface veryl_testcase_Interface80C
    import veryl_testcase_Package80::Command;
;


    logic   command_ready;
    logic   command_valid;
    Command command      ;
    logic   status_ready ;
    logic   status_valid ;
    logic   status       ;

    modport master_mp (
        input  command_ready,
        output command_valid,
        output command      ,
        output status_ready ,
        input  status_valid ,
        input  status       
    );

    modport slave_mp (
        output command_ready,
        input  command_valid,
        input  command      ,
        input  status_ready ,
        output status_valid ,
        output status       
    );
endinterface

module veryl_testcase_Module80A (
    veryl_testcase_Interface80A.mp command_if,
    veryl_testcase_Interface80B.mp status_if 
);
    veryl_testcase_Interface80C bus_if ();

    always_comb begin
        begin
            command_if.command_ready = bus_if.command_ready;
            bus_if.command_valid     = command_if.command_valid;
            bus_if.command           = command_if.command;
        end
        begin
            bus_if.status_ready      = status_if.status_ready;
            status_if.status_valid   = bus_if.status_valid;
            status_if.status         = veryl_testcase_Package80::Status'(bus_if.status);
        end
    end

    always_comb begin
        begin
            bus_if.command_ready = 0;
            bus_if.status_valid  = 0;
            bus_if.status        = 0;
        end
    end
endmodule

module veryl_testcase_Module80B (
    veryl_testcase_Interface80A.mp command_if,
    veryl_testcase_Interface80B.mp status_if 
);
    veryl_testcase_Interface80C bus_if ();

    always_comb begin
        command_if.command_ready = bus_if.command_ready;
        bus_if.command_valid     = command_if.command_valid;
        bus_if.command           = command_if.command;
    end
    always_comb begin
        bus_if.status_ready    = status_if.status_ready;
        status_if.status_valid = bus_if.status_valid;
        status_if.status       = veryl_testcase_Package80::Status'(bus_if.status);
    end

    always_comb begin
        bus_if.command_ready = 0;
        bus_if.status_valid  = 0;
        bus_if.status        = 0;
    end
endmodule
//# sourceMappingURL=../map/80_connect_operation.sv.map
