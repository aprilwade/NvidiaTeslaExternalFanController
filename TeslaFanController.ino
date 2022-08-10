#include "HID-Project.h"

uint8_t rawhidData[255];

word pwmA = 80 * 1; // 25% duty (0-320 = 0-100% duty cycle)
// word pwmB = 288; // 90% duty (0-320 = 0-100% duty cycle)

void setup() {
    pinMode(9, OUTPUT);  //pwmA
    pinMode(10, OUTPUT); //pwmB

    TCCR1A = 0;            //clear timer registers
    TCCR1B = 0;
    TCNT1 = 0;

    TCCR1B |= _BV(CS10);   //no prescaler
    ICR1 = 320;            //PWM mode counts up 320 then down 320 counts (25kHz)

    OCR1A = pwmA;          //0-320 = 0-100% duty cycle
    TCCR1A |= _BV(COM1A1); //output A clear rising/set falling

    OCR1B = pwmA;          //0-320 = 0-100% duty cycle
    TCCR1A |= _BV(COM1B1); //output B clear rising/set falling

    TCCR1B |= _BV(WGM13);  //PWM mode with ICR1 Mode 10
    TCCR1A |= _BV(WGM11);  //WGM13:WGM10 set 1010

    Serial.begin(9600);
    RawHID.begin(rawhidData, sizeof(rawhidData));
    RawHID.write((uint8_t)0);
}


void loop() {
    uint8_t buf_len = 0;
    uint8_t buf[64];

    // We expect to receive messages 64-bytes at a time.
    while (buf_len < 64) {
        int temp = RawHID.read();
        if (temp != -1) {
            buf[buf_len++] = temp;
        }
    }

    if (buf[0] == 1) {
        // Raw fan speed message (0-255)
        uint16_t speed = ((float)buf[1]) * 320.0 / 255.0;
        OCR1A = speed;          //0-320 = 0-100% duty cycle
        OCR1B = speed;          //0-320 = 0-100% duty cycle
        Serial.print("New speed ");
        Serial.print(speed);
        Serial.print("\n");
#endif
    }  else {
        Serial.print("Unsupported message: ");
        Serial.print(buf[0]);
        Serial.print("\n");
    }
}
